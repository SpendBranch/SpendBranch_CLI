//! Codex reader, against temporary folders only (never the real ~/.codex).
use serde_json::{Value, json};
use spendbranch::contract::{BillingMode, CoverageState, GapReason, Iso};
use spendbranch::infra::store::{Commit, SpoolLimits, Store};
use spendbranch::readers::codex::{Config, read};
use spendbranch::sources::{Source, outcome::ReadOutcome};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

fn now() -> Iso {
    Iso::parse("2026-06-01T12:00:00Z").unwrap()
}

struct Rig {
    root: tempfile::TempDir,
    store: Store,
    _data: tempfile::TempDir,
    max_archive_output: u64,
}

impl Rig {
    fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        Rig {
            root: tempfile::tempdir().unwrap(),
            store,
            _data: data,
            max_archive_output: 1 << 20,
        }
    }
    fn config(&self) -> Config {
        Config {
            root: self.root.path().to_path_buf(),
            max_line: 1 << 20,
            max_archive_output: self.max_archive_output,
        }
    }
    fn session(&self, rel: &str) -> PathBuf {
        self.root.path().join("sessions").join(rel)
    }
    fn archived(&self, name: &str) -> PathBuf {
        self.root.path().join("archived_sessions").join(name)
    }
    fn collect(&mut self) -> ReadOutcome {
        let outcome = read(
            &self.config(),
            &self.store.reader_state(Source::Codex).unwrap(),
            now(),
        );
        assert_eq!(
            self.store
                .commit_read(&outcome, SpoolLimits::default())
                .unwrap(),
            Commit::Stored
        );
        outcome
    }
}

const THREAD: &str = "thread_synth_1";

/// A synthetic rollout with `n` responses. Records from `context_until` on have no turn
/// context line of their own, like a long turn answered in several responses.
/// Building more records only appends: `rollout(a)` is a prefix of `rollout(b)` for a < b.
fn rollout(thread: &str, n: usize, context_until: usize) -> String {
    let meta = json!({"id": thread, "cli_version": "0.160.0", "model_provider": "openai",
        "creator_account_id": "acct_synth", "git": {"repository_url": "https://github.com/Acme/Portal.git", "branch": "main"}});
    let mut lines = vec![
        json!({"timestamp": "2026-06-01T10:00:00Z", "type": "session_meta", "ordinal": 0, "payload": meta}),
    ];
    let mut ordinal = 1;
    let mut push = |kind: &str, payload: Value, lines: &mut Vec<Value>| {
        lines.push(json!({"timestamp": format!("2026-06-01T10:{:02}:00Z", ordinal % 60), "type": kind, "ordinal": ordinal, "payload": payload}));
        ordinal += 1;
    };
    push(
        "event_msg",
        json!({"type": "token_count", "rate_limits": {"plan_type": "plus"}}),
        &mut lines,
    );
    let mut total = [0u64; 6];
    for i in 0..n {
        if i < context_until {
            push(
                "turn_context",
                json!({"turn_id": format!("turn_{i}"), "model": "gpt-test-1"}),
                &mut lines,
            );
        }
        let usage = [100 + i as u64, 10, 0, 20 + i as u64, 0, 120 + 2 * i as u64];
        for (t, u) in total.iter_mut().zip(usage) {
            *t += u;
        }
        let keys = [
            "input_tokens",
            "cached_input_tokens",
            "cache_write_input_tokens",
            "output_tokens",
            "reasoning_output_tokens",
            "total_tokens",
        ];
        let as_obj = |v: [u64; 6]| {
            Value::Object(
                keys.iter()
                    .zip(v)
                    .map(|(k, n)| (k.to_string(), json!(n)))
                    .collect(),
            )
        };
        let turn = if i < context_until {
            format!("turn_{i}")
        } else {
            format!("turn_{}", context_until.saturating_sub(1))
        };
        push(
            "token_usage_record",
            json!({"thread_id": thread, "turn_id": turn, "response_id": format!("resp_{thread}_{i}"),
            "usage": as_obj(usage), "thread_token_usage": as_obj(total)}),
            &mut lines,
        );
    }
    lines.iter().map(|l| format!("{l}\n")).collect()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn append(path: &Path, text: &str) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

fn zst(text: &str) -> Vec<u8> {
    zstd::encode_all(text.as_bytes(), 3).unwrap()
}

fn ids(o: &ReadOutcome) -> Vec<String> {
    o.observations
        .iter()
        .map(|x| x.external_event_id.as_str().to_owned())
        .collect()
}

fn gaps(o: &ReadOutcome) -> Vec<GapReason> {
    o.coverage
        .iter()
        .filter_map(|c| match c.state() {
            CoverageState::Unavailable { reason } => Some(reason),
            CoverageState::Observed => None,
        })
        .collect()
}

#[test]
fn a_first_read_then_nothing_new() {
    let mut rig = Rig::new();
    write(
        &rig.session("2026/06/01/rollout-a.jsonl"),
        &rollout(THREAD, 3, 3),
    );
    let first = rig.collect();
    assert_eq!(ids(&first).len(), 3);
    let o = &first.observations[0];
    assert_eq!(
        o.repository.as_ref().unwrap().as_str(),
        "github.com/acme/portal"
    );
    assert_eq!(o.billing_mode, Some(BillingMode::Subscription));
    let second = rig.collect();
    assert!(
        second.observations.is_empty() && second.cursor_set.is_empty() && gaps(&second).is_empty()
    );
    assert_eq!(rig.store.pending_counts().unwrap().0, 3);
}

#[test]
fn only_the_appended_part_is_read_and_it_keeps_the_threads_context() {
    let mut rig = Rig::new();
    let file = rig.session("2026/06/01/rollout-a.jsonl");
    write(&file, &rollout(THREAD, 2, 2));
    assert_eq!(ids(&rig.collect()).len(), 2);
    let all = rollout(THREAD, 4, 2); // records 2 and 3 carry no turn context of their own
    let first_part = rollout(THREAD, 2, 2).len();
    append(&file, &all[first_part..]);
    let out = rig.collect();
    assert_eq!(
        ids(&out),
        [format!("resp_{THREAD}_2"), format!("resp_{THREAD}_3")]
    );
    for o in &out.observations {
        assert_eq!(
            o.model.as_str(),
            "gpt-test-1",
            "the model comes from the saved context"
        );
        assert_eq!(o.payer_account_ref.as_ref().unwrap().as_str(), "acct_synth");
        assert_eq!(o.branch.as_ref().unwrap().as_str(), "main");
        assert_eq!(o.billing_mode, Some(BillingMode::Subscription));
    }
    assert!(
        out.coverage
            .iter()
            .all(|c| c.state() == CoverageState::Observed),
        "no gap for the appended part"
    );
    assert_eq!(rig.store.pending_counts().unwrap().0, 4);
}

#[test]
fn a_resumed_read_looks_only_at_the_appended_bytes() {
    let mut rig = Rig::new();
    let file = rig.session("rollout-a.jsonl");
    // a corrupt line near the top is a gap (1 of 7 lines is more than 5%) the first time ...
    let with_junk = |n: usize| {
        let text = rollout(THREAD, n, n);
        let (head, rest) = text.split_once('\n').unwrap();
        format!("{head}\nnot json at all\n{rest}")
    };
    write(&file, &with_junk(2));
    assert_eq!(gaps(&rig.collect()), [GapReason::UnsupportedFormat]);
    // ... and must not be seen again when only the new lines are read
    let first_len = with_junk(2).len();
    append(&file, &with_junk(5)[first_len..]);
    let out = rig.collect();
    assert_eq!(ids(&out).len(), 3);
    assert!(gaps(&out).is_empty(), "the old bytes were not read again");
    let cursor = &rig.store.reader_state(Source::Codex).unwrap().cursors
        [&format!("file:{}", file.display())];
    let cursor: Value = serde_json::from_str(cursor).unwrap();
    assert_eq!(
        cursor["offset"].as_u64(),
        Some(fs::metadata(&file).unwrap().len())
    );
}

#[test]
fn a_last_line_still_being_written_waits_for_its_line_break() {
    let mut rig = Rig::new();
    let file = rig.session("rollout-a.jsonl");
    let full = rollout(THREAD, 3, 3);
    write(&file, &full[..full.len() - 40]);
    assert_eq!(ids(&rig.collect()).len(), 2);
    append(&file, &full[full.len() - 40..]);
    assert_eq!(ids(&rig.collect()), [format!("resp_{THREAD}_2")]);
}

#[test]
fn codex_archive_moves_and_compresses_a_file_without_resending_anything() {
    let mut rig = Rig::new();
    let file = rig.session("2026/06/01/rollout-a.jsonl");
    write(&file, &rollout(THREAD, 2, 2));
    assert_eq!(ids(&rig.collect()).len(), 2);
    // `codex archive`: the plain file goes away, a compressed copy (now with two more
    // responses written before archiving) appears elsewhere
    fs::remove_file(&file).unwrap();
    fs::create_dir_all(rig.archived("x").parent().unwrap()).unwrap();
    fs::write(
        rig.archived("rollout-a.jsonl.zst"),
        zst(&rollout(THREAD, 4, 4)),
    )
    .unwrap();
    let out = rig.collect();
    assert_eq!(
        ids(&out),
        [format!("resp_{THREAD}_2"), format!("resp_{THREAD}_3")],
        "only what the thread had not sent"
    );
    assert!(gaps(&out).is_empty());
    assert!(
        rig.collect().observations.is_empty(),
        "and the archive is not read again"
    );
}

#[test]
fn an_archive_nobody_read_before_is_read_in_full_once() {
    let mut rig = Rig::new();
    fs::create_dir_all(rig.archived("x").parent().unwrap()).unwrap();
    fs::write(
        rig.archived("rollout-b.jsonl.zst"),
        zst(&rollout("thread_other", 3, 3)),
    )
    .unwrap();
    assert_eq!(ids(&rig.collect()).len(), 3);
    let again = rig.collect();
    assert!(again.observations.is_empty() && gaps(&again).is_empty());
}

#[test]
fn an_archive_that_expands_too_much_is_a_gap_not_a_crash() {
    let mut rig = Rig::new();
    rig.max_archive_output = 200;
    fs::create_dir_all(rig.archived("x").parent().unwrap()).unwrap();
    fs::write(
        rig.archived("rollout-b.jsonl.zst"),
        zst(&rollout("thread_bomb", 50, 50)),
    )
    .unwrap();
    let out = rig.collect();
    assert!(out.observations.is_empty());
    assert_eq!(gaps(&out), [GapReason::AdapterError]);
    assert!(gaps(&rig.collect()).is_empty(), "reported once");
}

#[test]
fn a_rollout_rewritten_shorter_starts_over() {
    let mut rig = Rig::new();
    let file = rig.session("rollout-a.jsonl");
    write(&file, &rollout(THREAD, 4, 4));
    assert_eq!(ids(&rig.collect()).len(), 4);
    write(&file, &rollout(THREAD, 2, 2)); // shorter, same path
    assert_eq!(
        ids(&rig.collect()).len(),
        2,
        "re-sent: the server drops what it has"
    );
}

#[test]
fn a_rollout_rewritten_in_place_with_other_content_does_not_resume_in_the_middle() {
    let mut rig = Rig::new();
    let file = rig.session("rollout-a.jsonl");
    write(&file, &rollout(THREAD, 2, 2));
    rig.collect();
    let other = rollout("thread_replacement", 6, 6); // same inode, bigger, another thread
    let mut f = File::create(&file).unwrap();
    f.write_all(other.as_bytes()).unwrap();
    drop(f);
    let out = rig.collect();
    assert_eq!(ids(&out).len(), 6);
    assert!(
        out.observations
            .iter()
            .all(|o| o.external_session_id.as_str() == "thread_replacement")
    );
}

#[test]
fn an_unreadable_format_is_one_gap_then_stays_quiet() {
    let mut rig = Rig::new();
    let mut text = rollout(THREAD, 3, 3);
    text = text.replace("\"output_tokens\":20", "\"output_tokens\":21"); // breaks the running total
    write(&rig.session("rollout-a.jsonl"), &text);
    let first = rig.collect();
    assert!(first.observations.is_empty());
    assert_eq!(gaps(&first), [GapReason::UnsupportedFormat]);
    let second = rig.collect();
    assert!(
        gaps(&second).is_empty(),
        "same unchanged file: no repeated gap"
    );
    assert_eq!(
        second.unsupported_files.len(),
        1,
        "still known to be unreadable"
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_rollout_is_one_gap_and_is_read_once_it_opens_again() {
    use std::os::unix::fs::PermissionsExt;
    let mut rig = Rig::new();
    let file = rig.session("rollout-a.jsonl");
    write(&file, &rollout(THREAD, 2, 2));
    fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
    if File::open(&file).is_ok() {
        return; // running as root
    }
    assert_eq!(gaps(&rig.collect()), [GapReason::AdapterError]);
    assert!(gaps(&rig.collect()).is_empty(), "reported once");
    // a failure may be passing: the file is tried again, not given up on
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(ids(&rig.collect()).len(), 2);
    assert!(
        rig.store
            .reader_state(Source::Codex)
            .unwrap()
            .unsupported_files
            .is_empty()
    );
}

#[test]
fn only_rollout_files_are_ever_opened() {
    let mut rig = Rig::new();
    let readable = rollout(THREAD, 2, 2);
    for rel in [
        "auth.json",
        "config.toml",
        "history.jsonl",
        "sessions/notes.jsonl",
        "sessions/rollout-x.txt",
        "state.sqlite",
        "other/rollout-y.jsonl",
    ] {
        write(&rig.root.path().join(rel), &readable);
    }
    let out = rig.collect();
    assert!(out.observations.is_empty() && gaps(&out).is_empty());
}

#[test]
fn discovery_stops_four_levels_down() {
    let mut rig = Rig::new();
    write(
        &rig.session("a/b/c/d/rollout-ok.jsonl"),
        &rollout("thread_ok", 1, 1),
    );
    write(
        &rig.session("a/b/c/d/e/rollout-deep.jsonl"),
        &rollout("thread_deep", 1, 1),
    );
    let out = rig.collect();
    assert_eq!(out.observations.len(), 1);
    assert_eq!(
        out.observations[0].external_session_id.as_str(),
        "thread_ok"
    );
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_never_followed() {
    let mut rig = Rig::new();
    let outside = rig.root.path().join("elsewhere");
    write(
        &outside.join("rollout-leak.jsonl"),
        &rollout("thread_leak", 2, 2),
    );
    fs::create_dir_all(rig.session("")).unwrap();
    std::os::unix::fs::symlink(
        outside.join("rollout-leak.jsonl"),
        rig.session("rollout-link.jsonl"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, rig.session("linked-dir")).unwrap();
    assert!(rig.collect().observations.is_empty());
}

#[test]
fn nothing_is_saved_until_the_store_commits() {
    let mut rig = Rig::new();
    write(&rig.session("rollout-a.jsonl"), &rollout(THREAD, 2, 2));
    let lost = read(
        &rig.config(),
        &rig.store.reader_state(Source::Codex).unwrap(),
        now(),
    );
    assert_eq!(lost.observations.len(), 2);
    assert_eq!(rig.store.pending_counts().unwrap(), (0, 0));
    assert!(
        rig.store
            .reader_state(Source::Codex)
            .unwrap()
            .cursors
            .is_empty()
    );
    assert_eq!(ids(&rig.collect()).len(), 2);
    assert!(rig.collect().observations.is_empty());
    assert_eq!(rig.store.pending_counts().unwrap().0, 2);
}

#[test]
fn an_untested_version_is_remembered_between_reads() {
    let mut rig = Rig::new();
    let file = rig.session("rollout-a.jsonl");
    write(&file, &rollout(THREAD, 1, 1).replace("0.160.0", "0.200.0"));
    assert_eq!(rig.collect().untested_version.as_deref(), Some("0.200.0"));
    assert_eq!(
        rig.collect().untested_version.as_deref(),
        Some("0.200.0"),
        "carried over a read that found nothing new"
    );
}
