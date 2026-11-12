//! The lenient filter of the OTLP export, against the shared fixtures.
#[path = "common/fake_server.rs"]
mod fake_server;
#[path = "common/rig.rs"]
mod rig;

use rig::{payload, record};
use serde_json::{Value, json};
use spendbranch::sources::otlp;
use std::{fs, path::Path};

fn json_of(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn fixtures() -> Vec<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/otlp");
    let mut dirs: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    dirs.sort();
    dirs
}

// --- the filter -----------------------------------------------------------------------

#[test]
fn the_lenient_payload_is_exactly_what_the_shared_corpus_expects() {
    let cases = fixtures();
    assert!(cases.len() >= 10, "the corpus went missing");
    for dir in cases {
        let input = json_of(&dir.join("input.json"));
        let expected = json_of(&dir.join("expected.json"));
        let got = otlp::lenient(&input);
        assert_eq!(got.payload, expected["relayPayload"], "{}", dir.display());
    }
}

#[test]
fn nothing_but_the_declared_attributes_of_an_api_request_gets_through() {
    let body = payload(vec![
        record(
            "api_request",
            json!([
                {"key": "model", "value": {"stringValue": "m"}},
                {"key": "prompt", "value": {"stringValue": "SECRET-PROMPT"}},
                {"key": "user.email", "value": {"stringValue": "SECRET-EMAIL"}},
                {"key": "input_tokens", "value": {"intValue": 5}},
                {"key": "output_tokens", "value": {"doubleValue": 1.5}},
                {"key": "cache_read_tokens", "value": {"boolValue": true}},
                {"key": "session.id", "value": {"stringValue": "s", "intValue": "1"}},
            ]),
        ),
        record(
            "user_prompt",
            json!([{"key": "model", "value": {"stringValue": "SECRET-OTHER"}}]),
        ),
        json!("not a record"),
    ]);
    let f = otlp::lenient(&body);
    assert_eq!((f.kept, f.dropped, f.ignored), (1, 2, 0));
    let text = f.payload.to_string();
    for secret in ["SECRET", "user.email", "a@b.c", "resource\":{"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let attrs = f
        .payload
        .pointer("/resourceLogs/0/scopeLogs/0/logRecords/0/attributes")
        .unwrap();
    let keys: Vec<&str> = attrs
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["model", "input_tokens", "session.id"]);
    assert_eq!(
        attrs[2]["value"],
        json!({"stringValue": "s"}),
        "one kind of value per attribute"
    );
}

#[test]
fn the_name_comes_from_the_field_then_the_attribute_then_the_body() {
    let by_body = json!({"body": {"stringValue": "claude_code.api_request"}, "attributes": []});
    let by_attr =
        json!({"attributes": [{"key": "event.name", "value": {"stringValue": "api_request"}}]});
    let other = json!({"eventName": "tool_result", "attributes": [{"key": "event.name", "value": {"stringValue": "api_request"}}]});
    let f = otlp::lenient(&payload(vec![by_body, by_attr, other]));
    assert_eq!(
        (f.kept, f.dropped),
        (2, 1),
        "the field wins over the attribute"
    );
}

#[test]
fn text_over_256_characters_is_dropped_and_256_stays() {
    let long = "é".repeat(257);
    let fine = "é".repeat(256);
    let f = otlp::lenient(&payload(vec![record(
        "api_request",
        json!([{"key": "model", "value": {"stringValue": long}}, {"key": "query_source", "value": {"stringValue": fine}}]),
    )]));
    let attrs = f
        .payload
        .pointer("/resourceLogs/0/scopeLogs/0/logRecords/0/attributes")
        .unwrap();
    assert_eq!(attrs.as_array().unwrap().len(), 1);
    assert_eq!(attrs[0]["key"], "query_source");
}

#[test]
fn a_repeated_key_keeps_the_first_and_odd_numbers_and_times_are_dropped() {
    let f = otlp::lenient(&payload(vec![json!({
    "eventName": "api_request", "timeUnixNano": "12ab",
    "attributes": [
        {"key": "model", "value": {"stringValue": "first"}},
        {"key": "model", "value": {"stringValue": "second"}},
        {"key": "input_tokens", "value": {"intValue": "1e9"}},
        {"key": "output_tokens", "value": {"intValue": "-7"}},
        {"key": "cache_read_tokens", "value": {"intValue": "99999999999999999999"}},
        {"key": "request_id"},
        {"value": {"stringValue": "no key"}},
    ]})]));
    let r = f
        .payload
        .pointer("/resourceLogs/0/scopeLogs/0/logRecords/0")
        .unwrap();
    assert!(r.get("timeUnixNano").is_none());
    let attrs = r["attributes"].as_array().unwrap();
    assert_eq!(attrs.len(), 2);
    assert_eq!(attrs[0]["value"]["stringValue"], "first");
    assert_eq!(attrs[1]["value"]["intValue"], "-7");
}

#[test]
fn only_the_first_2000_records_are_read() {
    let many: Vec<Value> = (0..2005)
        .map(|_| record("api_request", json!([])))
        .collect();
    let f = otlp::lenient(&payload(many));
    assert_eq!((f.kept, f.ignored, f.dropped), (2000, 5, 0));
}

#[test]
fn whatever_the_shape_the_answer_is_a_well_formed_empty_payload() {
    for body in [
        json!(null),
        json!([]),
        json!("x"),
        json!({"resourceLogs": 3}),
        json!({"resourceLogs": [null, {"scopeLogs": "x"}, {"scopeLogs": [{"logRecords": {}}]}]}),
    ] {
        let f = otlp::lenient(&body);
        assert_eq!(f.kept, 0);
        assert_eq!(
            f.payload,
            json!({"resourceLogs": [{"scopeLogs": [{"logRecords": []}]}]}),
            "{body}"
        );
    }
}

#[test]
fn the_relay_filter_keeps_exactly_the_otlp_fields_the_owner_authorizes() {
    // `FIELDS` is what `enable` sends to the server; the filter's list must not drift from it.
    let declared: Vec<&str> = spendbranch::sources::claude_code::FIELDS
        .iter()
        .filter_map(|f| f.strip_prefix("otlp:"))
        .collect();
    assert_eq!(otlp::KEYS.to_vec(), declared);
}
