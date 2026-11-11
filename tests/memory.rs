//! A huge prompt line must not grow the parser's memory: conversation text is skipped,
//! never stored. One test only: it reads a process-wide allocation counter.
use spendbranch::sources::{claude_code, codex};

mod common;
use common::extra_peak;

#[global_allocator]
static ALLOC: common::Counting = common::Counting;

#[test]
fn a_40_mib_prompt_line_costs_the_parser_almost_no_memory() {
    // Escapes force the JSON reader down its slowest string path.
    let prompt = "SYNTHETIC PROMPT \\\"quoted\\\" \\n\\u00e9 ".repeat(1_300_000);
    assert!(prompt.len() > 40 << 20);
    let user = format!(
        r#"{{"type":"user","sessionId":"s1","timestamp":"2026-01-01T10:00:00Z","message":{{"role":"user","content":"{prompt}"}}}}"#
    );
    let blocks = format!(
        r#"{{"type":"user","sessionId":"s1","message":{{"content":[{{"type":"text","text":"{prompt}"}}]}}}}"#
    );
    let assistant = r#"{"type":"assistant","sessionId":"s1","requestId":"r1","timestamp":"2026-01-01T10:00:01Z","message":{"id":"m","model":"claude-test-1","usage":{"input_tokens":1,"output_tokens":2}}}"#;
    let claude = format!("{user}\n{blocks}\n{assistant}\n");

    let meta = r#"{"timestamp":"2026-01-01T10:00:00Z","type":"session_meta","ordinal":0,"payload":{"id":"t1"}}"#;
    let turn = r#"{"timestamp":"2026-01-01T10:00:01Z","type":"turn_context","ordinal":1,"payload":{"turn_id":"u1","model":"gpt-test-1"}}"#;
    let chat = format!(
        r#"{{"timestamp":"2026-01-01T10:00:02Z","type":"event_msg","ordinal":2,"payload":{{"type":"agent_message","last_assistant_message":"{prompt}"}}}}"#
    );
    let rec = r#"{"timestamp":"2026-01-01T10:00:03Z","type":"token_usage_record","ordinal":3,"payload":{"thread_id":"t1","turn_id":"u1","response_id":"r1","usage":{"input_tokens":5,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":0,"total_tokens":7}}}"#;
    let rollout = format!("{meta}\n{turn}\n{chat}\n{rec}\n");

    let limit = 256 * 1024;
    let (peak, c) = extra_peak(|| claude_code::read(&claude, &claude_code::ReadOptions::default()));
    assert_eq!(c.observations.len(), 1, "the real event is still read");
    assert!(
        peak < limit,
        "claude-code parser used {peak} extra bytes for a {} byte input",
        claude.len()
    );

    let (peak, x) = extra_peak(|| codex::read(&rollout, &codex::ReadOptions::default()));
    assert_eq!(x.observations.len(), 1);
    assert!(
        peak < limit,
        "codex parser used {peak} extra bytes for a {} byte input",
        rollout.len()
    );
}
