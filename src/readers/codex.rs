//! Reads Codex rollouts (`sessions/**/rollout-*.jsonl` and `archived_sessions/*.zst`).
//!
//! The cursor is per thread (so `codex archive`, which moves and compresses a file, does
//! not resend anything) and per `.jsonl` file (an offset plus what the thread's first lines
//! told us, so only the appended part is read). A `.zst` is read once, in full.
use super::{
    cursor_json,
    discover::{self, Rollout},
    identity,
    lines::{Lines, MAX_LINE, Next},
    mtime_ms, settle, tail_hash,
};
use crate::contract::{GapReason, Iso, Source};
use crate::env::{Env, agent_dir};
use crate::sources::{
    Format,
    codex::{Parser, ReadOptions, ReadResult, Resume},
    coverage::{failure_now, too_many_unsupported},
    outcome::{ReadOutcome, ReaderState},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

/// A `.zst` that decompresses to more than this is refused (it could be a zip bomb).
pub const MAX_ARCHIVE_OUTPUT: u64 = 64 * 1024 * 1024;

pub struct Config {
    pub root: PathBuf,
    pub max_line: usize,
    pub max_archive_output: u64,
}

impl Config {
    /// `$CODEX_HOME` or `~/.codex`. `None` when no home folder can be found.
    pub fn from_env(env: Env) -> Option<Self> {
        Some(Self {
            root: agent_dir(env, Source::Codex)?,
            max_line: MAX_LINE,
            max_archive_output: MAX_ARCHIVE_OUTPUT,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct FileCursor {
    size: u64,
    mtime_ms: i64,
    dev: u64,
    ino: u64,
    /// Where the next read starts (`.jsonl` only), what lies before it, and the thread's context.
    offset: u64,
    tail: u64,
    thread_id: Option<String>,
    resume: Option<Resume>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ThreadCursor {
    last_ordinal: i64,
    path: String,
    size: u64,
    mtime_ms: i64,
}

struct FileRead {
    result: ReadResult,
    resume: Option<Resume>,
    non_empty: usize,
    offset: u64,
    tail: u64,
}

struct Stat {
    size: u64,
    mtime_ms: i64,
    dev: u64,
    ino: u64,
}

/// Streams `input` into a parser, then settles which thread it was to learn the ordinal to skip.
fn stream<R: Read>(
    input: R,
    start: u64,
    max_line: usize,
    mut parser: Parser<'_>,
    path: &str,
    threads: &HashMap<String, ThreadCursor>,
    size: u64,
) -> io::Result<(FileRead, R)> {
    let mut reader = Lines::new(BufReader::with_capacity(64 * 1024, input), start, max_line);
    let (mut buf, mut non_empty) = (Vec::new(), 0usize);
    while let Next::Line { too_long } = reader.next(&mut buf)? {
        non_empty += 1;
        if too_long {
            parser.push_unsupported_line();
        } else {
            let line = String::from_utf8_lossy(&buf);
            non_empty -= usize::from(line.is_empty());
            parser.push_line(&line);
        }
    }
    let offset = reader.complete_end;
    let after = parser
        .thread_id()
        .and_then(|t| threads.get(t))
        .map_or(-1, |t| {
            let shrunk = t.path == path && size < t.size;
            if shrunk { -1 } else { t.last_ordinal }
        });
    parser.set_after_ordinal(after);
    let (result, resume) = parser.finish();
    let inner = reader.into_inner().into_inner();
    Ok((
        FileRead {
            result,
            resume,
            non_empty,
            offset,
            tail: 0,
        },
        inner,
    ))
}

fn read_file(
    config: &Config,
    path: &Path,
    archived: bool,
    stat: &Stat,
    known: Option<&FileCursor>,
    threads: &HashMap<String, ThreadCursor>,
) -> io::Result<FileRead> {
    let options = ReadOptions::default();
    let key = path.to_string_lossy();
    let mut file = File::open(path)?;
    if archived {
        let decoder = zstd::stream::read::Decoder::new(file)?;
        let limited = decoder.take(config.max_archive_output + 1);
        let (read, limited) = stream(
            limited,
            0,
            config.max_line,
            Parser::new(&options),
            &key,
            threads,
            stat.size,
        )?;
        if limited.limit() == 0 {
            return Err(io::Error::other("archive expands beyond the limit"));
        }
        return Ok(read);
    }
    // Continue from the saved offset only if this is the same file, still holding the bytes
    // we read last time, and we kept the thread's context.
    let same = known.filter(|k| k.dev == stat.dev && k.ino == stat.ino && k.size <= stat.size);
    let resume = same
        .filter(|k| k.offset > 0 && k.offset <= stat.size)
        .filter(|k| tail_hash(&mut file, k.offset).ok() == Some(k.tail))
        .and_then(|k| k.resume.clone().map(|r| (k.offset, r)));
    let (start, parser) = match resume {
        Some((offset, state)) => (offset, Parser::resume(&options, state)),
        None => (0, Parser::new(&options)),
    };
    file.seek(SeekFrom::Start(start))?;
    let (mut read, mut file) = stream(
        file,
        start,
        config.max_line,
        parser,
        &key,
        threads,
        stat.size,
    )?;
    read.tail = if read.offset == 0 {
        0
    } else {
        tail_hash(&mut file, read.offset)?
    };
    Ok(read)
}

/// The cursor of a file that has not been read yet.
fn blank(stat: &Stat) -> FileCursor {
    FileCursor {
        size: stat.size,
        mtime_ms: stat.mtime_ms,
        dev: stat.dev,
        ino: stat.ino,
        offset: 0,
        tail: 0,
        thread_id: None,
        resume: None,
    }
}

/// One pass over the rollouts: the cursors being rebuilt, and what was found.
struct Scan<'a> {
    config: &'a Config,
    state: &'a ReaderState<'a>,
    now: Iso,
    out: ReadOutcome,
    threads: HashMap<String, ThreadCursor>,
    cursors: HashMap<String, String>,
    unsupported: Vec<String>,
    untested: Option<String>,
}

impl<'a> Scan<'a> {
    fn new(config: &'a Config, state: &'a ReaderState<'a>, now: Iso) -> Self {
        let threads = state
            .cursors
            .iter()
            .filter_map(|(k, v)| {
                Some((
                    k.strip_prefix("thread:")?.to_owned(),
                    serde_json::from_str(v).ok()?,
                ))
            })
            .collect();
        Self {
            config,
            state,
            now,
            out: ReadOutcome::empty(Source::Codex, now),
            threads,
            cursors: HashMap::new(),
            unsupported: Vec::new(),
            untested: state.untested_version.clone(),
        }
    }

    fn rollout(&mut self, rollout: &Rollout) {
        let path = rollout.path.to_string_lossy().into_owned();
        let key = format!("file:{path}");
        let Ok(meta) = fs::metadata(&rollout.path) else {
            return;
        };
        let (dev, ino) = identity(&meta);
        let stat = Stat {
            size: meta.len(),
            mtime_ms: mtime_ms(&meta),
            dev,
            ino,
        };
        let known: Option<FileCursor> = self
            .state
            .cursors
            .get(&key)
            .and_then(|j| serde_json::from_str(j).ok());
        if let Some(k) = known
            .as_ref()
            .filter(|k| k.size == stat.size && k.mtime_ms == stat.mtime_ms)
        {
            self.cursors.insert(key, cursor_json(k));
            if self.state.unsupported_files.contains(&path) {
                self.unsupported.push(path); // already reported: no repeated gap
            }
            return;
        }
        match read_file(
            self.config,
            &rollout.path,
            rollout.archived,
            &stat,
            known.as_ref(),
            &self.threads,
        ) {
            Ok(file) => self.accept(file, key, path, &stat, known.as_ref()),
            Err(_) => self.failed(rollout.archived, key, path, &stat),
        }
    }

    fn failed(&mut self, archived: bool, key: String, path: String, stat: &Stat) {
        if !self.state.unsupported_files.contains(&path) {
            self.out.coverage.push(failure_now(
                Source::Codex,
                self.now,
                GapReason::AdapterError,
            ));
        }
        // A plain rollout is tried again next time (the failure may be passing). An archive
        // that cannot be decompressed stays as it is: trying again would repeat expensive work
        // for the same result.
        if archived {
            self.cursors.insert(key, cursor_json(&blank(stat)));
        }
        self.unsupported.push(path);
    }

    fn accept(
        &mut self,
        file: FileRead,
        key: String,
        path: String,
        stat: &Stat,
        known: Option<&FileCursor>,
    ) {
        let FileRead {
            result,
            resume,
            non_empty,
            offset,
            tail,
        } = file;
        let mut cursor = FileCursor {
            thread_id: result.thread_id.clone(),
            offset,
            tail,
            resume,
            ..blank(stat)
        };
        let hard = result.format == Format::UnsupportedFormat;
        if hard || too_many_unsupported(result.unsupported_lines.len(), non_empty) {
            self.out.coverage.push(failure_now(
                Source::Codex,
                self.now,
                GapReason::UnsupportedFormat,
            ));
            self.unsupported.push(path.clone());
        }
        if hard {
            // Not understood: do not advance. A later change re-reads from the old offset.
            (cursor.offset, cursor.tail, cursor.resume) = match known {
                Some(k) => (k.offset, k.tail, k.resume.clone()),
                None => (0, 0, None),
            };
            self.cursors.insert(key, cursor_json(&cursor));
            return;
        }
        self.cursors.insert(key, cursor_json(&cursor));
        self.untested = result.untested_version.clone().or(self.untested.take());
        self.out.observations.extend(result.observations);
        if let Some(thread) = result.thread_id {
            self.advance_thread(thread, result.last_ordinal, path, stat);
        }
    }

    /// The thread's cursor moves to the highest ordinal seen, unless its file shrank (then the
    /// old ordinal means nothing).
    fn advance_thread(&mut self, thread: String, last_ordinal: i64, path: String, stat: &Stat) {
        let shrunk = self
            .threads
            .get(&thread)
            .is_some_and(|t| t.path == path && stat.size < t.size);
        let before = self
            .threads
            .get(&thread)
            .filter(|_| !shrunk)
            .map_or(-1, |t| t.last_ordinal);
        self.threads.insert(
            thread,
            ThreadCursor {
                last_ordinal: before.max(last_ordinal),
                path,
                size: stat.size,
                mtime_ms: stat.mtime_ms,
            },
        );
    }

    fn finish(mut self) -> ReadOutcome {
        for (thread, cursor) in self.threads {
            self.cursors
                .insert(format!("thread:{thread}"), cursor_json(&cursor));
        }
        settle(&mut self.out, &self.state.cursors, self.cursors);
        self.out.unsupported_files = self.unsupported;
        self.out.untested_version = self.untested;
        self.out
    }
}

/// Everything new in the rollouts since `state`, ready for `Store::commit_read`.
pub fn read(config: &Config, state: &ReaderState, now: Iso) -> ReadOutcome {
    let mut scan = Scan::new(config, state, now);
    for rollout in discover::codex_rollouts(&config.root) {
        scan.rollout(&rollout);
    }
    scan.finish()
}
