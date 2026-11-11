//! Reads Claude Code transcripts (`<projects>/**/*.jsonl`) incrementally.
use super::{
    Env, account, discover,
    git::RepoResolver,
    identity,
    lines::{Lines, MAX_LINE, Next},
    mtime_ms, tail_hash, user_home,
};
use crate::contract::{
    AccountRef, AgentRole, BillingMode, CoverageReport, CoverageState, GapReason, Iso,
    PayerEvidence, Repository, Source, UsageObservation,
};
use crate::sources::{
    Format,
    claude_code::{Parser, ReadOptions},
    coverage::{failure_now, too_many_unsupported, with_observed},
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
        let (projects_dir, account_file) = match env("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty())
        {
            Some(dir) => {
                let dir = PathBuf::from(dir);
                (dir.join("projects"), dir.join(".claude.json"))
            }
            None => {
                let h = home.clone()?;
                (h.join(".claude").join("projects"), h.join(".claude.json"))
            }
        };
        Some(Self {
            projects_dir,
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

struct Held {
    bytes: Vec<u8>,
    too_long: bool,
    start: u64,
}

fn read_file(
    path: &std::path::Path,
    known: Option<&FileCursor>,
    role: AgentRole,
    seen: &HashSet<u64>,
    now: Iso,
    max_line: usize,
) -> io::Result<Option<FileRead>> {
    let Ok(meta) = fs::metadata(path) else {
        return Ok(None);
    }; // vanished: drop its cursor
    let (dev, ino) = identity(&meta);
    let (size, mtime) = (meta.len(), mtime_ms(&meta));
    let same_file = known.is_some_and(|k| k.dev == dev && k.ino == ino);
    let unchanged = |offset: u64, tail: u64| FileRead {
        cursor: FileCursor {
            dev,
            ino,
            size,
            mtime_ms: mtime,
            offset,
            tail,
        },
        fed: 0,
        non_empty: 0,
        result: None,
        cwd: None,
    };
    if let Some(k) = known.filter(|_| same_file)
        && k.size == size
        && k.mtime_ms == mtime
        && k.offset == size
    {
        return Ok(Some(unchanged(k.offset, k.tail)));
    }

    let mut file = File::open(path)?;
    let mut start = known.filter(|_| same_file).map_or(0, |k| k.offset);
    if size < start
        || (start > 0 && known.is_none_or(|k| tail_hash(&mut file, start).ok() != Some(k.tail)))
    {
        start = 0; // shrunk or rewritten in place
    }
    if size == start {
        return Ok(Some(unchanged(
            start,
            if start == 0 {
                0
            } else {
                known.map_or(0, |k| k.tail)
            },
        )));
    }

    file.seek(SeekFrom::Start(start))?;
    let options = ReadOptions {
        subagent_file: role == AgentRole::Subagent,
        seen: Some(seen),
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

    // The lines of the last message stay pending until a different message follows: a file
    // still being written would freeze a smaller counter if we sent them now.
    let mut reader = Lines::new(BufReader::with_capacity(64 * 1024, file), start, max_line);
    let (mut pending, mut run_key): (Vec<Held>, Option<String>) = (Vec::new(), None);
    let mut buf = Vec::new();
    loop {
        let line_start = reader.complete_end;
        let Next::Line { too_long } = reader.next(&mut buf)? else {
            break;
        };
        let key = if too_long {
            None
        } else {
            request_key(&String::from_utf8_lossy(&buf)).map(str::to_owned)
        };
        let held = Held {
            bytes: buf.clone(),
            too_long,
            start: line_start,
        };
        let same_run = match (&run_key, &key) {
            (Some(run), Some(k)) => k == run,
            (Some(_), None) => true, // a line with no key stays with the open message
            (None, _) => false,
        };
        if run_key.is_none() && key.is_none() {
            feed(&buf, too_long);
        } else if same_run {
            pending.push(held);
        } else {
            for h in pending.drain(..) {
                feed(&h.bytes, h.too_long);
            }
            run_key = key;
            pending.push(held);
        }
    }
    let mut end = reader.complete_end;
    if now.millis() - mtime < OPEN_MESSAGE_MS
        && let Some(first) = pending.first()
    {
        end = first.start;
    } else {
        for h in pending.drain(..) {
            feed(&h.bytes, h.too_long);
        }
    }
    let mut file = reader.into_inner().into_inner();
    let tail = if end == 0 {
        0
    } else {
        tail_hash(&mut file, end)?
    };
    Ok(Some(FileRead {
        cursor: FileCursor {
            dev,
            ino,
            size,
            mtime_ms: mtime,
            offset: end,
            tail,
        },
        fed,
        non_empty,
        result: Some(parser.finish()),
        cwd,
    }))
}

/// Everything new in the transcripts since `state`, ready for `Store::commit_read`.
pub fn read(config: &Config, state: &ReaderState, now: Iso) -> ReadOutcome {
    let mut out = ReadOutcome::empty(Source::ClaudeCode, now);
    let mut seen = state.seen.clone();
    let local = account::read_local_account(&config.account_file);
    let mut repos = RepoResolver::new(config.home.clone());
    let listed = discover::claude_transcripts(&config.projects_dir);
    let mut cursors: HashMap<String, String> = HashMap::new();
    let (mut unsupported, mut reread) = (Vec::<String>::new(), HashSet::<String>::new());
    let mut untested = None;

    for (path, role) in &listed {
        let key = path.to_string_lossy().into_owned();
        let known: Option<FileCursor> = state
            .cursors
            .get(&key)
            .and_then(|j| serde_json::from_str(j).ok());
        let file = match read_file(path, known.as_ref(), *role, &seen, now, config.max_line) {
            Ok(Some(f)) => f,
            Ok(None) => continue,
            Err(_) => {
                if !state.unsupported_files.contains(&key) {
                    out.coverage.push(failure_now(
                        Source::ClaudeCode,
                        now,
                        GapReason::AdapterError,
                    ));
                }
                unsupported.push(key);
                continue;
            }
        };
        cursors.insert(
            key.clone(),
            serde_json::to_string(&file.cursor).expect("plain data"),
        );
        let Some(result) = file.result.filter(|_| file.fed > 0) else {
            continue;
        };
        reread.insert(key.clone());
        untested = result.untested_version.clone().or(untested);
        let mut times: Vec<Iso> = result.observations.iter().map(|o| o.occurred_at).collect();
        times.sort();
        if result.format == Format::UnsupportedFormat
            || too_many_unsupported(result.unsupported_lines.len(), file.non_empty)
        {
            unsupported.push(key.clone());
            let second_ago = Iso::from_millis((now.millis() - 1000).max(0)).unwrap_or(now);
            let from = times.first().copied().unwrap_or(second_ago);
            let to = times
                .last()
                .copied()
                .filter(|t| Some(t) > times.first())
                .unwrap_or(now);
            let gap = CoverageState::Unavailable {
                reason: GapReason::UnsupportedFormat,
            };
            out.coverage.push(
                CoverageReport::new(Source::ClaudeCode, from, to, gap).unwrap_or_else(|| {
                    failure_now(Source::ClaudeCode, now, GapReason::UnsupportedFormat)
                }),
            );
        }
        let repository = file
            .cwd
            .as_deref()
            .and_then(|cwd| repos.resolve(cwd))
            .and_then(|r| Repository::new(&r));
        for mut observation in result.observations {
            let hash = key_hash(observation.external_event_id.as_str());
            seen.insert(hash);
            out.seen.push((hash, observation.occurred_at));
            observation.repository = repository.clone();
            with_account(&mut observation, local.as_ref());
            out.observations.push(observation);
        }
    }

    let present: HashSet<String> = listed
        .iter()
        .map(|(p, _)| p.to_string_lossy().into_owned())
        .collect();
    for path in &state.unsupported_files {
        if present.contains(path) && !reread.contains(path) && !unsupported.contains(path) {
            unsupported.push(path.clone());
        }
    }
    let times: Vec<Iso> = out.observations.iter().map(|o| o.occurred_at).collect();
    out.coverage = with_observed(Source::ClaudeCode, out.coverage, &times, now);
    out.cursor_remove = state
        .cursors
        .keys()
        .filter(|k| !cursors.contains_key(*k))
        .cloned()
        .collect();
    out.cursor_remove.sort();
    out.cursor_set = cursors
        .into_iter()
        .filter(|(k, v)| state.cursors.get(k) != Some(v))
        .collect();
    out.cursor_set.sort();
    out.unsupported_files = unsupported;
    out.untested_version = untested;
    out
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
