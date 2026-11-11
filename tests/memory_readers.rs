//! Reading a transcript with a huge prompt line must stay within a few MiB: the line is
//! skipped while streaming, never loaded whole. One test only (process-wide counter).
use spendbranch::contract::Iso;
use spendbranch::readers::{claude_code, codex};
use spendbranch::sources::outcome::ReaderState;
use std::{fs, io::Write};

mod common;
use common::extra_peak;

#[global_allocator]
static ALLOC: common::Counting = common::Counting;

fn big_line(prefix: &str, suffix: &str, mib: usize, f: &mut fs::File) {
    f.write_all(prefix.as_bytes()).unwrap();
    let chunk = "SYNTHETIC PROMPT \\\"quoted\\\" ".repeat(4096);
    for _ in 0..(mib << 20) / chunk.len() {
        f.write_all(chunk.as_bytes()).unwrap();
    }
    f.write_all(suffix.as_bytes()).unwrap();
    f.write_all(b"\n").unwrap();
}

#[test]
fn a_64_mib_transcript_with_huge_lines_is_read_in_a_few_mib() {
    let dir = tempfile::tempdir().unwrap();
    let now = Iso::parse("2026-06-01T12:00:00Z").unwrap();

    let projects = dir.path().join("claude/projects/p");
    fs::create_dir_all(&projects).unwrap();
    let mut f = fs::File::create(projects.join("s1.jsonl")).unwrap();
    let assistant = |r: &str| {
        format!(
            r#"{{"type":"assistant","sessionId":"s1","requestId":"{r}","timestamp":"2026-06-01T10:00:00Z","message":{{"id":"m","model":"claude-test-1","usage":{{"input_tokens":1,"output_tokens":2}}}}}}"#
        )
    };
    writeln!(f, "{}", assistant("r1")).unwrap();
    big_line(
        r#"{"type":"user","sessionId":"s1","message":{"content":""#,
        r#""}}"#,
        32,
        &mut f,
    );
    big_line(
        r#"{"type":"user","sessionId":"s1","message":{"content":""#,
        r#""}}"#,
        32,
        &mut f,
    );
    writeln!(f, "{}", assistant("r2")).unwrap();
    drop(f);
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    fs::OpenOptions::new()
        .write(true)
        .open(projects.join("s1.jsonl"))
        .unwrap()
        .set_modified(old)
        .unwrap();

    let sessions = dir.path().join("codex/sessions");
    fs::create_dir_all(&sessions).unwrap();
    let mut f = fs::File::create(sessions.join("rollout-a.jsonl")).unwrap();
    writeln!(f, r#"{{"timestamp":"2026-06-01T10:00:00Z","type":"session_meta","ordinal":0,"payload":{{"id":"t1"}}}}"#).unwrap();
    writeln!(f, r#"{{"timestamp":"2026-06-01T10:00:01Z","type":"turn_context","ordinal":1,"payload":{{"turn_id":"u1","model":"gpt-test-1"}}}}"#).unwrap();
    big_line(
        r#"{"timestamp":"2026-06-01T10:00:02Z","type":"event_msg","ordinal":2,"payload":{"type":"agent_message","last_assistant_message":""#,
        r#""}}"#,
        32,
        &mut f,
    );
    writeln!(f, r#"{{"timestamp":"2026-06-01T10:00:03Z","type":"token_usage_record","ordinal":3,"payload":{{"thread_id":"t1","turn_id":"u1","response_id":"p1","usage":{{"input_tokens":5,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":0,"total_tokens":7}}}}}}"#).unwrap();
    drop(f);

    let claude_cfg = claude_code::Config {
        projects_dir: dir.path().join("claude/projects"),
        account_file: dir.path().join("claude/.claude.json"),
        home: None,
        max_line: spendbranch::readers::lines::MAX_LINE,
    };
    let (peak, out) = extra_peak(|| claude_code::read(&claude_cfg, &ReaderState::default(), now));
    assert_eq!(
        out.observations.len(),
        2,
        "the real events around the huge lines are read"
    );
    assert!(
        peak < 16 << 20,
        "claude reader used {peak} extra bytes for a 64 MiB file"
    );

    let codex_cfg = codex::Config {
        root: dir.path().join("codex"),
        max_line: spendbranch::readers::lines::MAX_LINE,
        max_archive_output: codex::MAX_ARCHIVE_OUTPUT,
    };
    let (peak, out) = extra_peak(|| codex::read(&codex_cfg, &ReaderState::default(), now));
    assert_eq!(out.observations.len(), 1);
    assert!(
        peak < 16 << 20,
        "codex reader used {peak} extra bytes for a 32 MiB file"
    );
}
