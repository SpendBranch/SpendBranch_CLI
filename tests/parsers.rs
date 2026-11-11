//! Parser parity: every fixture must give exactly the expected output, deterministically.
use serde_json::{Value, json};
use spendbranch::sources::{claude_code, codex, key_hash, repository::repository_key_of, version};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

fn fixtures(tool: &str) -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(tool);
    let mut v: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

fn json_file(p: &Path) -> Option<Value> {
    fs::read_to_string(p)
        .ok()
        .map(|t| serde_json::from_str(&t).unwrap())
}

fn seen_of(options: &Value) -> Option<HashSet<u64>> {
    options["seen"]
        .as_array()
        .map(|a| a.iter().map(|k| key_hash(k.as_str().unwrap())).collect())
}

fn run_claude(dir: &Path) -> Value {
    let text = fs::read_to_string(dir.join("input.jsonl")).unwrap();
    let options = json_file(&dir.join("options.json")).unwrap_or(Value::Null);
    let seen = seen_of(&options);
    let opts = claude_code::ReadOptions {
        subagent_file: options["role"] == "subagent",
        seen: seen.as_ref(),
    };
    let first = claude_code::read(&text, &opts);
    assert_eq!(
        first,
        claude_code::read(&text, &opts),
        "{}: not deterministic",
        dir.display()
    );
    serde_json::to_value(first).unwrap()
}

fn run_codex(dir: &Path) -> Value {
    let text = fs::read_to_string(dir.join("input.jsonl")).unwrap();
    let options = json_file(&dir.join("options.json")).unwrap_or(Value::Null);
    let seen = seen_of(&options);
    let opts = codex::ReadOptions {
        after_ordinal: options["afterOrdinal"].as_i64(),
        seen: seen.as_ref(),
    };
    let first = codex::read(&text, &opts);
    assert_eq!(
        first,
        codex::read(&text, &opts),
        "{}: not deterministic",
        dir.display()
    );
    serde_json::to_value(first).unwrap()
}

#[test]
fn claude_code_fixtures_match_the_reference() {
    let dirs = fixtures("claude-code");
    assert!(dirs.len() >= 14);
    for dir in dirs {
        let expected = json_file(&dir.join("expected.json")).unwrap();
        assert_eq!(run_claude(&dir), expected, "{}", dir.display());
    }
}

#[test]
fn codex_fixtures_match_the_reference() {
    let dirs = fixtures("codex");
    assert!(dirs.len() >= 17);
    for dir in dirs {
        let expected = json_file(&dir.join("expected.json")).unwrap();
        assert_eq!(run_codex(&dir), expected, "{}", dir.display());
    }
}

#[test]
fn repository_keys_match_the_reference() {
    let cases = json_file(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repository/cases.json"),
    )
    .unwrap();
    for c in cases.as_array().unwrap() {
        let input = c["input"].as_str().unwrap();
        assert_eq!(json!(repository_key_of(input)), c["expected"], "{input:?}");
    }
}

#[test]
fn allowlists_are_byte_identical_to_the_authorized_lists() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fields");
    for (name, fields) in [
        ("claude-code", &claude_code::FIELDS[..]),
        ("codex", &codex::FIELDS[..]),
    ] {
        let expected = fs::read_to_string(dir.join(format!("{name}.txt"))).unwrap();
        assert_eq!(format!("{}\n", fields.join("\n")), expected, "{name}");
    }
}

#[test]
fn parser_versions_match_the_contract() {
    assert_eq!(claude_code::PARSER, "claude-code/v2");
    assert_eq!(codex::PARSER, "codex/v1");
}

#[test]
fn tested_version_ranges() {
    for (v, ok) in [
        ("2.1.290", true),
        ("2.1.320", true),
        ("2.1.300-beta", true),
        ("2.1.289", false),
        ("2.1.321", false),
        ("2.2.0", false),
        ("3.0.0", false),
        ("garbage", false),
    ] {
        assert_eq!(
            version::in_range(v, &version::CLAUDE_CODE_TESTED),
            ok,
            "claude {v}"
        );
    }
    for (v, ok) in [
        ("0.155.0", true),
        ("0.165.999", true),
        ("0.154.9", false),
        ("0.166.0", false),
    ] {
        assert_eq!(
            version::in_range(v, &version::CODEX_TESTED),
            ok,
            "codex {v}"
        );
    }
}

#[test]
fn key_hash_is_stable_because_it_is_persisted() {
    assert_eq!(key_hash(""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(key_hash("a"), 0xaf63_dc4c_8601_ec8c);
}

#[test]
fn reading_again_with_the_new_keys_as_seen_emits_nothing() {
    let dir = &fixtures("claude-code")[0];
    let text = fs::read_to_string(dir.join("input.jsonl")).unwrap();
    let first = claude_code::read(&text, &claude_code::ReadOptions::default());
    let seen: HashSet<u64> = first.new_keys.iter().map(|k| key_hash(k)).collect();
    assert!(!first.observations.is_empty());
    let again = claude_code::read(
        &text,
        &claude_code::ReadOptions {
            seen: Some(&seen),
            ..Default::default()
        },
    );
    assert!(again.observations.is_empty() && again.new_keys.is_empty());
}

#[test]
fn nothing_but_allowlisted_metadata_reaches_the_output() {
    for tool in ["claude-code", "codex"] {
        for dir in fixtures(tool) {
            let out = if tool == "codex" {
                run_codex(&dir)
            } else {
                run_claude(&dir)
            }
            .to_string();
            for leak in [
                "SYNTHETIC PROMPT",
                "tok_FIXTURE",
                "synthetic-project",
                "/home/",
            ] {
                assert!(!out.contains(leak), "{}: leaks {leak}", dir.display());
            }
        }
    }
}

#[test]
fn a_rollout_read_in_two_parts_gives_what_one_read_gives() {
    use codex::{Parser, ReadOptions};
    let opts = ReadOptions::default();
    for name in [
        "basic",
        "model-switch",
        "subagent",
        "foreign-record",
        "broken-line-skips-step-check",
        "untested-version",
    ] {
        let text = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/codex")
                .join(name)
                .join("input.jsonl"),
        )
        .unwrap();
        let whole = codex::read(&text, &opts);
        let lines: Vec<&str> = text
            .split_inclusive('\n')
            .map(|l| l.trim_end_matches('\n'))
            .collect();
        let mut checked = 0;
        for split in 1..lines.len() {
            let mut first = Parser::new(&opts);
            lines[..split].iter().for_each(|l| first.push_line(l));
            let (a, resume) = first.finish();
            let Some(resume) = resume else { continue }; // that prefix is not readable on its own
            let mut second = Parser::resume(&opts, resume);
            lines[split..].iter().for_each(|l| second.push_line(l));
            let (b, _) = second.finish();
            // The billing mode is learned from a plan event that may come after the records it
            // applies to, so a read cut before that event cannot know it: compare the rest.
            let plain = |o: &spendbranch::contract::UsageObservation| {
                let mut o = o.clone();
                o.billing_mode = None;
                o
            };
            let joined: Vec<_> = a
                .observations
                .iter()
                .chain(&b.observations)
                .map(plain)
                .collect();
            let expected: Vec<_> = whole.observations.iter().map(plain).collect();
            assert_eq!(joined, expected, "{name} split at line {split}");
            assert_eq!(
                b.last_ordinal, whole.last_ordinal,
                "{name} split at line {split}"
            );
            checked += 1;
        }
        assert!(checked > 0, "{name}: no split point was usable");
    }
}

#[test]
fn a_resume_state_survives_being_saved_as_json() {
    use codex::{Parser, ReadOptions, Resume};
    let opts = ReadOptions::default();
    let text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/basic/input.jsonl"),
    )
    .unwrap();
    let mut p = Parser::new(&opts);
    text.lines().for_each(|l| p.push_line(l));
    let (_, resume) = p.finish();
    let resume = resume.unwrap();
    let again: Resume = serde_json::from_str(&serde_json::to_string(&resume).unwrap()).unwrap();
    assert_eq!(again, resume);
}
