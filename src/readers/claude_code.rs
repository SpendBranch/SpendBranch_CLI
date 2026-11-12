//! Reads Claude Code transcripts (`<projects>/**/*.jsonl`) incrementally.
use super::{
    account, cursor_json, discover,
    git::RepoResolver,
    hold::OpenMessage,
    identity,
    lines::{Lines, MAX_LINE, Next},
    mtime_ms, settle, tail_hash,
};
use crate::contract::{
    AccountRef, AgentRole, BillingMode, CoverageReport, CoverageState, GapReason, Iso,
    PayerEvidence, Repository, Source, UsageObservation,
};
use crate::env::{Env, agent_dir, user_home};
use crate::sources::{
    Format,
    claude_code::{Parser, ReadOptions},
    coverage::{failure_now, too_many_unsupported},
    key_hash,
    outcome::{ReadOutcome, ReaderState},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, BufReader, Seek, SeekFrom},
    path::PathBuf,
};

/// A file modified more recently than this may still be growing its last message.
const OPEN_MESSAGE_MS: i64 = 30_000;

pub struct Config {
    pub projects_dir: PathBuf,
    pub account_file: PathBuf,
    pub home: Option<PathBuf>,
    pub max_line: usize,
}

impl Config {
    /// `$CLAUDE_CONFIG_DIR` or `~/.claude`. `None` when no home folder can be found.
    pub fn from_env(env: Env) -> Option<Self> {
        let home = user_home(env);
        let dir = agent_dir(env, Source::ClaudeCode)?;
        // With the folder chosen, the account file is inside it; by default it sits in `~`.
        let account_file = if env("CLAUDE_CONFIG_DIR").is_some_and(|d| !d.is_empty()) {
            dir.join(".claude.json")
        } else {
            home.clone()?.join(".claude.json")
        };
        Some(Self {
            projects_dir: dir.join("projects"),
            account_file,
            home,
            max_line: MAX_LINE,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct FileCursor {
    dev: u64,
    ino: u64,
    size: u64,
    mtime_ms: i64,
    offset: u64,
    /// Fingerprint of the bytes just before `offset`.
    tail: u64,
}

struct FileRead {
    cursor: FileCursor,
    /// Lines handed to the parser, and how many of them were not blank.
    fed: usize,
    non_empty: usize,
    result: Option<crate::sources::claude_code::ReadResult>,
    cwd: Option<String>,
}

/// `"requestId":"<id>"` anywhere in the line: the key of the message the line belongs to.
fn request_key(line: &str) -> Option<&str> {
    let rest = &line[line.find("\"requestId\":\"")? + 13..];
    let id = &rest[..rest.find('"')?];
    (!id.is_empty()).then_some(id)
}

/// The first `"cwd":"..."` of a line. It only feeds the repository lookup, then is dropped.
fn cwd_of(line: &str) -> Option<String> {
    let start = line.find("\"cwd\":\"")? + 6;
    let rest = &line[start..];
    let mut escaped = false;
    for (i, c) in rest.char_indices().skip(1) {
        match c {
            '\\' => escaped = !escaped,
            '"' if !escaped => return serde_json::from_str(&rest[..=i]).ok(),
            _ => escaped = false,
        }
    }
    None
}

/// What the file looks like now.
struct Stat {
    dev: u64,
    ino: u64,
    size: u64,
    mtime_ms: i64,
}

impl Stat {
    fn cursor(&self, offset: u64, tail: u64) -> FileCursor {
        FileCursor {
            dev: self.dev,
            ino: self.ino,
            size: self.size,
            mtime_ms: self.mtime_ms,
            offset,
            tail,
        }
    }

    /// Nothing new to read: the cursor only learns the file's present size and time.
    fn quiet(&self, offset: u64, tail: u64) -> FileRead {
        FileRead {
            cursor: self.cursor(offset, tail),
            fed: 0,
            non_empty: 0,
            result: None,
            cwd: None,
        }
    }
}

/// Where to start reading: at the saved offset if this is the same file and still holds the
/// bytes read last time, else at the beginning (it shrank or was rewritten in place).
fn start_of(file: &mut File, known: Option<&FileCursor>, same_file: bool, size: u64) -> u64 {
    let start = known.filter(|_| same_file).map_or(0, |k| k.offset);
    let lost = size < start
        || (start > 0 && known.is_none_or(|k| tail_hash(file, start).ok() != Some(k.tail)));
    if lost { 0 } else { start }
}

/// How a read goes on once there is something new.
struct Pass<'a> {
    role: AgentRole,
    seen: &'a dyn Fn(u64) -> bool,
    /// The last message may still be growing: it is held back.
    recent: bool,
    max_line: usize,
}

fn read_file(
    path: &std::path::Path,
    known: Option<&FileCursor>,
    role: AgentRole,
    seen: &dyn Fn(u64) -> bool,
    now: Iso,
    max_line: usize,
) -> io::Result<Option<FileRead>> {
    let Ok(meta) = fs::metadata(path) else {
        return Ok(None);
    }; // vanished: drop its cursor
    let (dev, ino) = identity(&meta);
    let stat = Stat {
        dev,
        ino,
        size: meta.len(),
        mtime_ms: mtime_ms(&meta),
    };
    let same_file = known.is_some_and(|k| k.dev == dev && k.ino == ino);
    if let Some(k) = known.filter(|_| same_file)
        && k.size == stat.size
        && k.mtime_ms == stat.mtime_ms
        && k.offset == stat.size
    {
        return Ok(Some(stat.quiet(k.offset, k.tail)));
    }
    let mut file = File::open(path)?;
    let start = start_of(&mut file, known, same_file, stat.size);
    if stat.size == start {
        let tail = if start == 0 {
            0
        } else {
            known.map_or(0, |k| k.tail)
        };
        return Ok(Some(stat.quiet(start, tail)));
    }
    // A modification time well ahead of the clock (the clock was set back, a file was copied)
    // says nothing about the age. Then only a file that changed since the last read is still
    // growing: one that did not change has settled, and its last message is sent.
    let changed =
        known.is_none_or(|k| !same_file || k.size != stat.size || k.mtime_ms != stat.mtime_ms);
    let recent = match now.millis() - stat.mtime_ms {
        age if age < -OPEN_MESSAGE_MS => changed,
        age => age < OPEN_MESSAGE_MS,
    };
    let pass = Pass {
        role,
        seen,
        recent,
        max_line,
    };
    stream_new(file, start, &stat, &pass).map(Some)
}

/// Feeds the new lines to the parser, holding back a message that may still be growing.
fn stream_new(mut file: File, start: u64, stat: &Stat, pass: &Pass) -> io::Result<FileRead> {
    file.seek(SeekFrom::Start(start))?;
    let options = ReadOptions {
        subagent_file: pass.role == AgentRole::Subagent,
        seen: Some(pass.seen),
    };
    let mut parser = Parser::new(&options);
    let (mut fed, mut non_empty, mut cwd) = (0usize, 0usize, None::<String>);
    let mut feed = |bytes: &[u8], too_long: bool| {
        fed += 1;
        if too_long {
            non_empty += 1;
            parser.push_unsupported_line();
            return;
        }
        let line = String::from_utf8_lossy(bytes);
        non_empty += usize::from(!line.is_empty());
        if cwd.is_none() {
            cwd = cwd_of(&line);
        }
        parser.push_line(&line);
    };
    let mut reader = Lines::new(
        BufReader::with_capacity(64 * 1024, file),
        start,
        pass.max_line,
    );
    let mut open_message = OpenMessage::default();
    let mut buf = Vec::new();
    loop {
        let line_start = reader.complete_end;
        let Next::Line { too_long } = reader.next(&mut buf)? else {
            break;
        };
        let text = String::from_utf8_lossy(&buf);
        let key = if too_long { None } else { request_key(&text) };
        open_message.push(&buf, too_long, line_start, key, &mut feed);
    }
    let end = open_message
        .finish(pass.recent, &mut feed)
        .unwrap_or(reader.complete_end);
    let mut file = reader.into_inner().into_inner();
    let tail = if end == 0 {
        0
    } else {
        tail_hash(&mut file, end)?
    };
    Ok(FileRead {
        cursor: stat.cursor(end, tail),
        fed,
        non_empty,
        result: Some(parser.finish()),
        cwd,
    })
}

/// One pass over the transcripts: the cursors being rebuilt, and what was found.
struct Scan<'a> {
    config: &'a Config,
    state: &'a ReaderState<'a>,
    now: Iso,
    out: ReadOutcome,
    /// Events found in this very read: a forked session copies history into other files.
    fresh: HashSet<u64>,
    local: Option<account::LocalAccount>,
    repos: RepoResolver,
    cursors: HashMap<String, String>,
    unsupported: Vec<String>,
    reread: HashSet<String>,
    untested: Option<String>,
}

impl<'a> Scan<'a> {
    fn new(config: &'a Config, state: &'a ReaderState<'a>, now: Iso) -> Self {
        Self {
            config,
            state,
            now,
            out: ReadOutcome::empty(Source::ClaudeCode, now),
            fresh: HashSet::new(),
            local: account::read_local_account(&config.account_file),
            repos: RepoResolver::new(config.home.clone()),
            cursors: HashMap::new(),
            unsupported: Vec::new(),
            reread: HashSet::new(),
            untested: None,
        }
    }

    fn transcript(&mut self, path: &std::path::Path, role: AgentRole) {
        let key = path.to_string_lossy().into_owned();
        let known: Option<FileCursor> = self
            .state
            .cursors
            .get(&key)
            .and_then(|j| serde_json::from_str(j).ok());
        let (fresh, state) = (&self.fresh, self.state);
        let is_seen = |hash: u64| fresh.contains(&hash) || state.is_seen(hash);
        let file = match read_file(
            path,
            known.as_ref(),
            role,
            &is_seen,
            self.now,
            self.config.max_line,
        ) {
            Ok(Some(f)) => f,
            Ok(None) => return,
            Err(_) => {
                if !self.state.unsupported_files.contains(&key) {
                    self.out.coverage.push(failure_now(
                        Source::ClaudeCode,
                        self.now,
                        GapReason::AdapterError,
                    ));
                }
                self.unsupported.push(key);
                return;
            }
        };
        self.cursors.insert(key.clone(), cursor_json(&file.cursor));
        let Some(result) = file.result.filter(|_| file.fed > 0) else {
            return;
        };
        self.reread.insert(key.clone());
        self.untested = result.untested_version.clone().or(self.untested.take());
        if result.format == Format::UnsupportedFormat
            || too_many_unsupported(result.unsupported_lines.len(), file.non_empty)
        {
            self.unsupported.push(key);
            self.gap(&result.observations);
        }
        let repository = file
            .cwd
            .as_deref()
            .and_then(|cwd| self.repos.resolve(cwd))
            .and_then(|r| Repository::new(&r));
        for mut observation in result.observations {
            let hash = key_hash(observation.external_event_id.as_str());
            self.fresh.insert(hash);
            self.out.seen.push((hash, observation.occurred_at));
            observation.repository = repository.clone();
            with_account(&mut observation, self.local.as_ref());
            self.out.observations.push(observation);
        }
    }

    /// A file that could not be understood: the period it covers is unavailable, not zero.
    fn gap(&mut self, observations: &[UsageObservation]) {
        let mut times: Vec<Iso> = observations.iter().map(|o| o.occurred_at).collect();
        times.sort();
        let second_ago = Iso::from_millis((self.now.millis() - 1000).max(0)).unwrap_or(self.now);
        let from = times.first().copied().unwrap_or(second_ago);
        let to = times
            .last()
            .copied()
            .filter(|t| Some(t) > times.first())
            .unwrap_or(self.now);
        let gap = CoverageState::Unavailable {
            reason: GapReason::UnsupportedFormat,
        };
        self.out.coverage.push(
            CoverageReport::new(Source::ClaudeCode, from, to, gap).unwrap_or_else(|| {
                failure_now(Source::ClaudeCode, self.now, GapReason::UnsupportedFormat)
            }),
        );
    }

    /// A gap already reported for a file that is still there and was not read again stays.
    fn finish(mut self, listed: &[(std::path::PathBuf, AgentRole)]) -> ReadOutcome {
        let present: HashSet<String> = listed
            .iter()
            .map(|(p, _)| p.to_string_lossy().into_owned())
            .collect();
        for path in &self.state.unsupported_files {
            if present.contains(path)
                && !self.reread.contains(path)
                && !self.unsupported.contains(path)
            {
                self.unsupported.push(path.clone());
            }
        }
        settle(&mut self.out, &self.state.cursors, self.cursors);
        self.out.unsupported_files = self.unsupported;
        self.out.untested_version = self.untested;
        self.out
    }
}

/// Everything new in the transcripts since `state`, ready for `Store::commit_read`.
pub fn read(config: &Config, state: &ReaderState, now: Iso) -> ReadOutcome {
    let mut scan = Scan::new(config, state, now);
    let listed = discover::claude_transcripts(&config.projects_dir);
    for (path, role) in &listed {
        scan.transcript(path, *role);
    }
    scan.finish(&listed)
}

/// A payer found in the transcript wins; otherwise the device's own account is used.
fn with_account(o: &mut UsageObservation, local: Option<&account::LocalAccount>) {
    let Some(local) = local else { return };
    if o.payer_account_ref.is_none() {
        o.payer_account_ref = AccountRef::new(&local.account);
        o.payer_evidence = Some(PayerEvidence::LocalAccount);
    }
    let is_local =
        o.payer_account_ref.as_ref().map(AccountRef::as_str) == Some(local.account.as_str());
    if local.subscription && is_local {
        o.billing_mode = Some(BillingMode::Subscription);
    }
}
