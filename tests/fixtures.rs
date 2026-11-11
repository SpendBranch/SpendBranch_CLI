//! Guards the F0 contract: the schema is valid, every fixture is complete and
//! every expected observation conforms to `schema/ingest-v1.json`.
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn json_of(p: &Path) -> Value {
    serde_json::from_str(&read(p)).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Case directories: tests/fixtures/<tool>/<case>/ (repository/ is a single file).
fn cases(tool: &str) -> Vec<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(tool);
    let mut v: Vec<_> = fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

fn schema() -> jsonschema::Validator {
    let s = json_of(&Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/ingest-v1.json"));
    jsonschema::validator_for(&s).expect("schema compiles")
}

#[test]
fn version_flag_prints_the_crate_version() {
    for flag in ["--version", "-v"] {
        let out = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
            .arg(flag)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap().trim(),
            env!("CARGO_PKG_VERSION")
        );
    }
}

#[test]
fn schema_accepts_a_minimal_batch_and_rejects_bad_ones() {
    let v = schema();
    let obs = json!({
        "source": "codex", "externalSessionId": "s", "externalEventId": "e",
        "occurredAt": "2026-01-01T00:00:00Z", "model": "m", "agentRole": "main",
        "counters": {"inputTokens": 1, "outputTokens": null, "cacheReadTokens": 0,
                     "cacheWriteTokens": 0, "reasoningTokens": 0},
        "counterOrigin": "reported_delta", "parserVersion": "codex/v1"
    });
    let batch = |o: Value| {
        json!({"contractVersion": "ingest/v1",
        "installationId": "123e4567-e89b-42d3-a456-426614174000", "observations": [o]})
    };
    assert!(v.is_valid(&batch(obs.clone())));
    let mut extra = obs.clone();
    extra["cwd"] = json!("/home/x"); // strict: unknown keys are refused
    assert!(!v.is_valid(&batch(extra)));
    let mut neg = obs;
    neg["counters"]["inputTokens"] = json!(-1);
    assert!(!v.is_valid(&batch(neg)));
}

#[test]
fn every_fixture_is_complete_and_its_output_matches_the_schema() {
    let v = schema();
    let mut total = 0;
    for tool in ["claude-code", "codex", "otlp"] {
        let input = if tool == "otlp" {
            "input.json"
        } else {
            "input.jsonl"
        };
        for dir in cases(tool) {
            let name = dir.display().to_string();
            assert!(dir.join(input).is_file(), "{name}: missing {input}");
            let expected = json_of(&dir.join("expected.json"));
            if dir.join("options.json").exists() {
                json_of(&dir.join("options.json"));
            }
            let obs = expected["observations"]
                .as_array()
                .unwrap_or_else(|| panic!("{name}: no observations"));
            // an empty list is valid output but not a valid batch: validate per observation
            for o in obs {
                let batch = json!({"contractVersion": "ingest/v1",
                    "installationId": "123e4567-e89b-42d3-a456-426614174000", "observations": [o]});
                assert!(
                    v.is_valid(&batch),
                    "{name}: observation violates ingest/v1: {o}"
                );
            }
            // privacy: nothing from the synthetic prompts, secrets, paths or e-mails may be in the output
            let text = read(&dir.join("expected.json"));
            for leak in [
                "SYNTHETIC PROMPT",
                "tok_FIXTURE",
                "example.invalid",
                "synthetic-project",
                "/home/",
            ] {
                assert!(!text.contains(leak), "{name}: output leaks {leak}");
            }
            total += 1;
        }
    }
    assert!(total >= 40, "fixture corpus shrank: {total}");
}

#[test]
fn corpus_covers_the_adversarial_cases() {
    let has = |tool: &str, case: &str| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(tool)
            .join(case)
            .is_dir()
    };
    for (tool, case) in [
        ("claude-code", "fork-seen"),       // fork
        ("claude-code", "non-usage-lines"), // compaction
        ("claude-code", "open-message"),    // open message
        ("codex", "after-ordinal"),         // rewritten / archived file re-read
        ("codex", "open-last-line"),        // open message
        ("codex", "conflicting-response"),
        ("codex", "thread-usage-mismatch"),
    ] {
        assert!(has(tool, case), "missing adversarial fixture {tool}/{case}");
    }
}

#[test]
fn repository_key_cases_are_well_formed() {
    let cases = json_of(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repository/cases.json"),
    );
    let cases = cases.as_array().unwrap();
    assert!(cases.len() >= 10);
    for c in cases {
        assert!(c["input"].is_string() && (c["expected"].is_string() || c["expected"].is_null()));
        assert!(
            !c["expected"].as_str().unwrap_or("").contains('@'),
            "credentials leaked: {c}"
        );
    }
}
