//! The lenient filter of the OTLP export, against the shared fixtures.
use crate::rig;

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
fn text_is_kept_only_in_the_shape_its_key_promises() {
    let attr = |k: &str, v: &str| json!({"key": k, "value": {"stringValue": v}});
    let f = otlp::lenient(&payload(vec![
        record(
            "api_request",
            json!([
                // free text, a path, a line break, over 256: never forwarded
                attr("model", &"é".repeat(257)),
                attr("session.id", "/home/dev/secret-project"),
                attr("request_id", "req 1\nSECRET"),
                attr("query_source", "SECRET PROMPT TEXT"),
                attr("organization.id", "org synth"),
                attr("event.timestamp", "2026-01-01 10:00:00 <script>"),
                attr("input_tokens", "SECRET"),
                json!({"key": "output_tokens", "value": {"stringValue": "12"}}),
            ]),
        ),
        record(
            "api_request",
            json!([
                attr("model", "claude-test-1"),
                attr("session.id", "sess_1"),
                attr("request_id", "req_1"),
                attr("query_source", "agent:explore"),
                attr("organization.id", "org_synth_1"),
                attr("event.timestamp", "2026-01-01T10:00:00-03:00"),
            ]),
        ),
    ]));
    let keys = |i: usize| -> Vec<String> {
        f.payload
            .pointer(&format!(
                "/resourceLogs/0/scopeLogs/0/logRecords/{i}/attributes"
            ))
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["key"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(
        keys(0),
        ["output_tokens"],
        "only the counter that is a number"
    );
    assert_eq!(keys(1).len(), 6, "{:?}", keys(1));
    // past the contract's ceiling for one counter (10^12), as text or as a number
    let over = otlp::lenient(&payload(vec![record(
        "api_request",
        json!([
            {"key": "input_tokens", "value": {"intValue": "1000000000001"}},
            {"key": "output_tokens", "value": {"intValue": 9_000_000_000_000_000u64}},
            {"key": "cache_read_tokens", "value": {"intValue": "1000000000000"}},
        ]),
    )]));
    let kept = over.payload["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["attributes"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(kept, 1, "only the counter at the ceiling stays");
    // below zero, as text, as a number or as a string value: never forwarded; zero is a count
    let below = otlp::lenient(&payload(vec![record(
        "api_request",
        json!([
            {"key": "input_tokens", "value": {"intValue": "-1"}},
            {"key": "output_tokens", "value": {"intValue": -1}},
            {"key": "cache_read_tokens", "value": {"stringValue": "-3"}},
            {"key": "cache_creation_tokens", "value": {"intValue": "0"}},
        ]),
    )]));
    let kept: Vec<_> =
        below.payload["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["key"].as_str().unwrap())
            .collect();
    assert_eq!(kept, ["cache_creation_tokens"], "only the zero stays");
    assert!(!f.payload.to_string().contains("SECRET"));
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
    assert_eq!(attrs.len(), 1);
    assert_eq!(attrs[0]["value"]["stringValue"], "first");
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

#[test]
fn walking_the_bytes_gives_exactly_what_walking_the_parsed_body_gives() {
    let mut bodies: Vec<String> = fixtures()
        .iter()
        .map(|d| fs::read_to_string(d.join("input.json")).unwrap())
        .collect();
    let rec = |extra: &str| {
        format!(r#"{{"resourceLogs":[{{"scopeLogs":[{{"logRecords":[{extra}]}}]}}]}}"#)
    };
    for odd in [
        "0, null, \"x\", [1,[2]], {}",
        r#"{"eventName":"","body":{"stringValue":"api_request"},"attributes":[{"key":"model","value":{"stringValue":"m1"}}]}"#,
        r#"{"eventName":7,"attributes":[{"key":"event.name","value":5},{"key":"event.name","value":{"stringValue":"api_request"}}]}"#,
        r#"{"attributes":[{"key":"event.name","value":{"stringValue":"api_request"}},{"key":"input_tokens","value":{"intValue":1.5}},{"key":"input_tokens","value":{"intValue":"7"}},{"key":"output_tokens","value":{"stringValue":"9","intValue":null}}]}"#,
        r#"{"eventName":"api_request","timeUnixNano":-5,"attributes":{"key":"model"}}"#,
        r#"{"eventName":"api_request","timeUnixNano":12,"attributes":[null,1,{"key":3,"value":{}},{"value":{"stringValue":"no key"}},{"key":"model"},{"key":"model","value":{"stringValue":"ok-model","x":[1,2,{"y":3}]}}]}"#,
        r#"{"eventName":"claude_code.api_request","attributes":[{"key":"session.id","value":{"stringValue":"s","stringValue":7}},{"key":"request_id","key":"model","value":{"stringValue":"r1"}}]}"#,
    ] {
        bodies.push(rec(odd));
    }
    bodies.push(rec(&vec![r#"{"eventName":"api_request"}"#; 2100].join(",")));
    bodies.push(format!(
        r#"{{"resourceLogs":[{{"scopeLogs":[{{"logRecords":[{}]}},{{"logRecords":[{}]}}]}},7,{{"scopeLogs":{{}}}}]}}"#,
        vec!["0"; 1999].join(","),
        [r#"{"eventName":"api_request"}"#; 3].join(",")
    ));
    for garbage in [
        "[]",
        "7",
        "\"x\"",
        "null",
        r#"{"resourceLogs":{}}"#,
        r#"{"resourceLogs":[[],{"scopeLogs":[[{}]]}]}"#,
    ] {
        bodies.push(garbage.into());
    }
    for body in &bodies {
        let parsed = otlp::lenient(&serde_json::from_str(body).unwrap());
        let walked = otlp::lenient_bytes(body.as_bytes()).expect(body);
        assert_eq!(walked.payload, parsed.payload, "{body}");
        assert_eq!(
            (walked.kept, walked.dropped, walked.ignored),
            (parsed.kept, parsed.dropped, parsed.ignored),
            "{body}"
        );
    }
    // Not JSON, something after it, or a list key given twice in one object: refused.
    for bad in [
        "{nope",
        "{} {}",
        r#"{"resourceLogs":[],"resourceLogs":[]}"#,
        r#"{"resourceLogs":[{"scopeLogs":[],"scopeLogs":[]}]}"#,
        r#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[],"logRecords":[]}]}]}"#,
    ] {
        assert!(otlp::lenient_bytes(bad.as_bytes()).is_none(), "{bad}");
    }
}
