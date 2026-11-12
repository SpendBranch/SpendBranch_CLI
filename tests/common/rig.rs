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
};
use spendbranch::sources::{claude_code, codex};
use std::{cell::RefCell, collections::HashMap, io::Cursor};

pub struct Out {
    pub code: i32,
    pub text: String,
    /// Every pause the command asked for, in seconds.
    pub sleeps: Vec<u64>,
    pub opened: Vec<String>,
}

pub struct Rig {
    pub dir: tempfile::TempDir,
    pub server: FakeServer,
}

impl Rig {
    pub fn new() -> Self {
        let server = FakeServer::start();
        server.configure(|s| s.script.approve_at_poll = Some(2));
        Self {
            dir: tempfile::tempdir().unwrap(),
            server,
        }
    }

    pub fn run_with(&self, args: &[&str], input: &str, tty: bool, extra: &[(&str, &str)]) -> Out {
        let mut env: HashMap<String, String> = [
            ("SPENDBRANCH_HOME", self.dir.path().to_str().unwrap()),
            ("SPENDBRANCH_KEY_STORE", "file"),
            ("SPENDBRANCH_SERVER", &self.server.base),
            ("HOME", self.dir.path().to_str().unwrap()),
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
        s.collection = vec![json!({"source": "claude_code", "lastSeenAt": null,
            "observedUntil": null, "openGaps": []})];
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
