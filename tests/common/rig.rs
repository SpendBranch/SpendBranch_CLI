//! The in-process harness shared by the journey tests: a fake server, a data folder, and a
//! `run` that gives the command its keyboard, sleep and browser. Nothing waits, opens a
//! browser or touches the real keychain.
#![allow(dead_code)]
use crate::fake_server::FakeServer;
use serde_json::json;
use spendbranch::cli::{self, Io};
use spendbranch::infra::{
    keys::{KeyFiles, KeyStore, Slot},
    store::Store,
    system::{Ran, System},
};
use spendbranch::sources::{claude_code, codex};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    io::{self, Cursor},
    path::{Path, PathBuf},
};

pub struct Out {
    pub code: i32,
    pub text: String,
    /// Every pause the command asked for, in seconds.
    pub sleeps: Vec<u64>,
    pub opened: Vec<String>,
}

/// Processes that never run: what was asked of them is recorded, and the tools answer as told
/// (everything succeeds with no output unless said otherwise).
/// (program, arguments, stdin)
pub type ToolCall = (String, Vec<String>, Option<String>);

pub struct FakeSystem {
    pub exe: PathBuf,
    pub stdin: RefCell<Option<Vec<u8>>>,
    pub stdin_reads: Cell<u32>,
    /// (program, arguments, log file) of every detached start.
    pub spawned: RefCell<Vec<(PathBuf, Vec<String>, PathBuf)>>,
    /// (program, arguments, stdin) of every tool run.
    pub ran: RefCell<Vec<ToolCall>>,
    /// Answers by the start of `"program arguments"`.
    pub answers: RefCell<HashMap<String, (i32, String)>>,
    /// Programs that do not exist on this machine.
    pub missing: RefCell<HashSet<String>>,
}

impl FakeSystem {
    pub fn new() -> Self {
        Self {
            exe: PathBuf::from("/opt/spend branch/spendbranch"),
            stdin: RefCell::new(None),
            stdin_reads: Cell::new(0),
            spawned: RefCell::default(),
            ran: RefCell::default(),
            answers: RefCell::default(),
            missing: RefCell::default(),
        }
    }

    pub fn answer(&self, key: &str, code: i32, stdout: &str) {
        self.answers
            .borrow_mut()
            .insert(key.to_owned(), (code, stdout.to_owned()));
    }

    pub fn tool_calls(&self, program: &str) -> Vec<Vec<String>> {
        self.ran
            .borrow()
            .iter()
            .filter(|(p, ..)| p == program)
            .map(|(_, a, _)| a.clone())
            .collect()
    }
}

impl System for FakeSystem {
    fn exe(&self) -> io::Result<PathBuf> {
        Ok(self.exe.clone())
    }
    fn stdin_brief(&self) -> Option<Vec<u8>> {
        self.stdin_reads.set(self.stdin_reads.get() + 1);
        self.stdin.borrow().clone()
    }
    fn spawn_detached(&self, exe: &Path, args: &[&str], log: &Path) -> io::Result<()> {
        self.spawned.borrow_mut().push((
            exe.to_path_buf(),
            args.iter().map(|a| a.to_string()).collect(),
            log.to_path_buf(),
        ));
        Ok(())
    }
    fn stopped(&self) -> bool {
        true
    }
    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> io::Result<Ran> {
        if self.missing.borrow().contains(program) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "not found"));
        }
        self.ran.borrow_mut().push((
            program.to_owned(),
            args.iter().map(|a| a.to_string()).collect(),
            stdin.map(str::to_owned),
        ));
        // The longest key that starts the command line wins.
        let line = format!("{program} {}", args.join(" "));
        let (code, stdout) = self
            .answers
            .borrow()
            .iter()
            .filter(|(k, _)| line.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map_or((0, String::new()), |(_, v)| v.clone());
        Ok(Ran {
            code,
            stdout,
            stderr: String::new(),
        })
    }
}

pub struct Rig {
    pub dir: tempfile::TempDir,
    pub server: FakeServer,
    pub system: FakeSystem,
}

impl Rig {
    pub fn new() -> Self {
        let server = FakeServer::start();
        server.configure(|s| s.script.approve_at_poll = Some(2));
        Self {
            dir: tempfile::tempdir().unwrap(),
            server,
            system: FakeSystem::new(),
        }
    }

    pub fn run_with(&self, args: &[&str], input: &str, tty: bool, extra: &[(&str, &str)]) -> Out {
        let mut env: HashMap<String, String> = [
            ("SPENDBRANCH_HOME", self.dir.path().to_str().unwrap()),
            ("SPENDBRANCH_KEY_STORE", "file"),
            ("SPENDBRANCH_SERVER", &self.server.base),
            ("HOME", self.dir.path().to_str().unwrap()),
            ("USERPROFILE", self.dir.path().to_str().unwrap()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let (sleeps, opened) = (RefCell::new(vec![]), RefCell::new(vec![]));
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let mut out = Vec::new();
        let mut keyboard = Cursor::new(input.as_bytes().to_vec());
        let code = cli::run_with(
            &args,
            &|k| env.get(k).cloned(),
            &mut Io {
                out: &mut out,
                input: &mut keyboard,
                sleep: &|d| sleeps.borrow_mut().push(d.as_secs()),
                open_url: &|u| opened.borrow_mut().push(u.to_owned()),
                interactive: tty,
                system: &self.system,
            },
        );
        Out {
            code,
            text: String::from_utf8(out).unwrap(),
            sleeps: sleeps.into_inner(),
            opened: opened.into_inner(),
        }
    }

    pub fn run(&self, args: &[&str]) -> Out {
        self.run_with(args, "y\n", false, &[])
    }

    pub fn store(&self) -> Store {
        Store::open(self.dir.path()).unwrap()
    }

    pub fn key_file(&self, slot: Slot) -> bool {
        KeyFiles::new(self.dir.path()).load(slot).unwrap().is_some()
    }

    pub fn paths_seen(&self) -> Vec<String> {
        self.server.seen().into_iter().map(|s| s.path).collect()
    }
}

pub fn authorize(rig: &Rig) {
    let granted = |source: &str, parser: &str, fields: &[&str]| {
        json!({"source": source, "state": "active", "parserVersion": parser,
               "fields": fields, "windowStart": "2026-11-01T00:00:00Z"})
    };
    rig.server.configure(|s| {
        s.sources = vec![
            granted("claude_code", claude_code::PARSER, &claude_code::FIELDS),
            granted("codex", codex::PARSER, &codex::FIELDS),
        ];
    });
}

/// A valid observation for `source`, in `session`, at `at` (ISO).
pub fn observation(
    source: &str,
    session: &str,
    event: &str,
    at: &str,
) -> spendbranch::contract::UsageObservation {
    let parser = if source == "codex" {
        "codex/v1"
    } else {
        "claude-code/v2"
    };
    serde_json::from_value(json!({
        "source": source, "externalSessionId": session, "externalEventId": event,
        "occurredAt": at, "model": "m", "agentRole": "main",
        "counters": {"inputTokens": 1, "outputTokens": 1, "cacheReadTokens": 0,
                     "cacheWriteTokens": 0, "reasoningTokens": 0},
        "counterOrigin": "reported_delta", "parserVersion": parser
    }))
    .unwrap()
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

impl Rig {
    /// Logged in, both sources authorized and turned on.
    pub fn ready() -> Self {
        let rig = Rig::new();
        authorize(&rig);
        assert_eq!(rig.run(&["login"]).code, 0);
        for source in ["claude-code", "codex"] {
            assert_eq!(rig.run(&["enable", source]).code, 0);
        }
        rig
    }

    /// Puts observations in the buffer as if a read at `read_at_ms` had found them.
    pub fn buffer(&self, read_at_ms: i64, items: Vec<spendbranch::contract::UsageObservation>) {
        use spendbranch::contract::{Iso, Source};
        use spendbranch::infra::store::SpoolLimits;
        use spendbranch::sources::outcome::ReadOutcome;
        let mut outcome = ReadOutcome::empty(Source::Codex, Iso::from_millis(read_at_ms).unwrap());
        outcome.observations = items;
        self.store()
            .commit_read(&outcome, SpoolLimits::default())
            .unwrap();
    }

    pub fn buffered(&self) -> u64 {
        self.store().pending_counts().unwrap().0
    }

    pub fn sync_state(&self, key: &str) -> Option<String> {
        self.store().sync_get(key).unwrap()
    }
}
