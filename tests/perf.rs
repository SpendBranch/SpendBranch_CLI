//! Performance budget. Slow, so ignored by default; run it in release mode:
//!
//! ```text
//! cargo test --release --test perf -- --ignored --nocapture
//! ```
//!
//! The thresholds are a safety net against gross regressions (they only apply to release
//! builds). The plan's goal is a first read of 1 GB of transcripts in under 10 s, which is
//! about 100 MiB/s; these floors are set at half of that.
use spendbranch::contract::Iso;
use spendbranch::infra::store::{SpoolLimits, Store};
use spendbranch::readers::{claude_code, codex};
use spendbranch::sources::{Source, outcome::ReadOutcome, outcome::ReaderState};
use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
    time::{Duration, Instant, UNIX_EPOCH},
};

const MIB: usize = 1 << 20;

fn now() -> Iso {
    Iso::parse("2026-06-01T12:00:00Z").unwrap()
}

fn age(path: &Path) {
    let old = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(old)
        .unwrap();
}

/// A Claude Code transcript where one line in ten carries usage and the rest is conversation.
fn claude_transcript(path: &Path, mib: usize) -> usize {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut out = BufWriter::new(fs::File::create(path).unwrap());
    let filler = "lorem ipsum dolor sit amet ".repeat(55);
    let (mut bytes, mut i) = (0usize, 0u64);
    while bytes < mib * MIB {
        let line = if i.is_multiple_of(10) {
            format!(
                r#"{{"type":"assistant","sessionId":"s1","requestId":"req_{i}","timestamp":"2026-06-01T10:00:00Z","version":"2.1.300","cwd":"/x","message":{{"id":"m{i}","model":"claude-test-1","content":[{{"type":"text","text":"{filler}"}}],"usage":{{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":40}}}}}}"#
            )
        } else {
            format!(
                r#"{{"type":"user","sessionId":"s1","timestamp":"2026-06-01T10:00:00Z","message":{{"role":"user","content":"{filler}"}}}}"#
            )
        };
        bytes += line.len() + 1;
        writeln!(out, "{line}").unwrap();
        i += 1;
    }
    out.flush().unwrap();
    age(path);
    bytes
}

/// A Codex rollout where one line in five is a usage record and the rest is conversation.
fn codex_rollout(path: &Path, mib: usize) -> usize {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut out = BufWriter::new(fs::File::create(path).unwrap());
    let filler = "lorem ipsum dolor sit amet ".repeat(55);
    writeln!(out, r#"{{"timestamp":"2026-06-01T10:00:00Z","type":"session_meta","ordinal":0,"payload":{{"id":"t1"}}}}"#).unwrap();
    writeln!(out, r#"{{"timestamp":"2026-06-01T10:00:00Z","type":"turn_context","ordinal":1,"payload":{{"turn_id":"u1","model":"gpt-test-1"}}}}"#).unwrap();
    let (mut bytes, mut ordinal, mut input, mut output) = (0usize, 2u64, 0u64, 0u64);
    while bytes < mib * MIB {
        let line = if ordinal.is_multiple_of(5) {
            (input, output) = (input + 100, output + 20);
            format!(
                r#"{{"timestamp":"2026-06-01T10:00:00Z","type":"token_usage_record","ordinal":{ordinal},"payload":{{"thread_id":"t1","turn_id":"u1","response_id":"p{ordinal}","usage":{{"input_tokens":100,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":20,"reasoning_output_tokens":0,"total_tokens":120}},"thread_token_usage":{{"input_tokens":{input},"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":{output},"reasoning_output_tokens":0,"total_tokens":{}}}}}}}"#,
                input + output
            )
        } else {
            format!(
                r#"{{"timestamp":"2026-06-01T10:00:00Z","type":"response_item","ordinal":{ordinal},"payload":{{"text":"{filler}"}}}}"#
            )
        };
        bytes += line.len() + 1;
        writeln!(out, "{line}").unwrap();
        ordinal += 1;
    }
    out.flush().unwrap();
    bytes
}

fn report(what: &str, bytes: usize, took: Duration) -> f64 {
    let rate = (bytes / MIB) as f64 / took.as_secs_f64();
    println!("{what}: {} MiB in {took:?} = {rate:.0} MiB/s", bytes / MIB);
    rate
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn reading_and_saving_stay_within_budget() {
    let dir = tempfile::tempdir().unwrap();

    let file = dir.path().join("claude/projects/p/s1.jsonl");
    let bytes = claude_transcript(&file, 400);
    let config = claude_code::Config {
        projects_dir: dir.path().join("claude/projects"),
        account_file: dir.path().join("none.json"),
        home: None,
        max_line: 4 * MIB,
    };
    let started = Instant::now();
    let claude = claude_code::read(&config, &ReaderState::default(), now());
    let claude_rate = report("claude-code read", bytes, started.elapsed());

    let data = tempfile::tempdir().unwrap();
    let mut store = Store::open(data.path()).unwrap();
    let started = Instant::now();
    store.commit_read(&claude, SpoolLimits::default()).unwrap();
    println!(
        "commit of {} observations: {:?}",
        claude.observations.len(),
        started.elapsed()
    );

    // 450 thousand remembered keys is what 45 days of heavy use leaves behind
    let mut many = ReadOutcome::empty(Source::ClaudeCode, now());
    many.seen = (0..450_000u64)
        .map(|k| (k.wrapping_mul(0x9E37_79B9_7F4A_7C15), now()))
        .collect();
    store.commit_read(&many, SpoolLimits::default()).unwrap();
    let started = Instant::now();
    let state = store.reader_state(Source::ClaudeCode).unwrap();
    let state_took = started.elapsed();
    println!(
        "reader_state with {} seen keys: {state_took:?}",
        store.seen_count(Source::ClaudeCode).unwrap()
    );
    drop(state);

    let rollout = dir.path().join("codex/sessions/rollout-a.jsonl");
    let bytes = codex_rollout(&rollout, 200);
    let config = codex::Config {
        root: dir.path().join("codex"),
        max_line: 4 * MIB,
        max_archive_output: codex::MAX_ARCHIVE_OUTPUT,
    };
    let started = Instant::now();
    let _ = codex::read(&config, &ReaderState::default(), now());
    let codex_rate = report("codex read", bytes, started.elapsed());

    if !cfg!(debug_assertions) {
        assert!(
            claude_rate > 50.0,
            "claude-code reads {claude_rate:.0} MiB/s"
        );
        assert!(codex_rate > 50.0, "codex reads {codex_rate:.0} MiB/s");
        assert!(
            state_took < Duration::from_millis(50),
            "reader_state took {state_took:?}: the seen keys must not be loaded"
        );
    }
}

/// The hook is the one thing the agent waits for: from start to exit, under 200 ms (the plan's
/// budget). Measured on the real binary with a connected device, so it also starts the
/// detached `sync` (which stops at once: there is no key).
#[test]
#[ignore = "slow; run in release mode with the others"]
fn the_hook_exits_in_under_200_ms() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(dir.path()).unwrap();
        store.meta_set("device_id", "dev-1").unwrap();
    }
    let body = br#"{"hook_event_name":"SessionEnd","session_id":"s1","transcript_path":"/x"}"#;
    let mut times = Vec::new();
    for _ in 0..15 {
        let started = Instant::now();
        let mut child = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
            .args(["hook", "claude-code"])
            .env("SPENDBRANCH_HOME", dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(body).unwrap();
        let out = child.wait_with_output().unwrap();
        times.push(started.elapsed());
        assert!(out.status.success());
        assert!(
            out.stdout.is_empty() && out.stderr.is_empty(),
            "the hook says nothing"
        );
    }
    times.sort();
    let (median, worst) = (times[times.len() / 2], times[times.len() - 1]);
    println!("hook: median {median:?}, worst {worst:?}");
    std::thread::sleep(Duration::from_millis(500)); // let the detached syncs finish
    if !cfg!(debug_assertions) {
        assert!(
            median < Duration::from_millis(200),
            "hook median {median:?}"
        );
    }
}
