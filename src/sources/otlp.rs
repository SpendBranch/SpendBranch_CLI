//! The lenient filter of Claude Code's OTLP log export (`claude-code-otlp/v1`): what the local
//! relay lets through. Pure. The server filters again, strictly; this one exists so that what
//! leaves the machine is only what the company authorized, even if the agent exports more.
//!
//! Kept: records of an `api_request` event, and of each record only its name, its time and the
//! twelve declared attributes. Resource attributes, the record body, every other attribute and
//! any text over 256 characters never pass. Not here: the mapping into observations, which the
//! server does.
use serde_json::{Map, Value};

/// Records read per request; the rest are ignored.
pub const MAX_RECORDS: usize = 2000;
/// Longer text is dropped, not cut.
pub const MAX_TEXT: usize = 256;

const EVENTS: [&str; 2] = ["api_request", "claude_code.api_request"];
/// The declared attributes: the `otlp:` entries of `claude_code::FIELDS`.
pub const KEYS: [&str; 12] = [
    "event.name",
    "event.timestamp",
    "session.id",
    "request_id",
    "model",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_creation_tokens",
    "cache_creation_1h_tokens",
    "query_source",
    "organization.id",
];

pub struct Filtered {
    /// What goes to the server: one resource, one scope, the kept records.
    pub payload: Value,
    pub kept: usize,
    /// Records of another event, or not a record.
    pub dropped: usize,
    /// Records past the 2000th.
    pub ignored: usize,
}

fn attributes(record: &Value) -> impl Iterator<Item = (&str, &Value)> {
    record
        .get("attributes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| Some((a.get("key")?.as_str()?, a.get("value")?)))
}

fn text_of(value: &Value) -> Option<&str> {
    value.get("stringValue")?.as_str()
}

/// The event's name: `eventName`, else the `event.name` attribute, else the body's text.
fn event_name(record: &Value) -> Option<&str> {
    record
        .get("eventName")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .or_else(|| {
            attributes(record)
                .find(|(k, _)| *k == "event.name")
                .and_then(|(_, v)| text_of(v))
        })
        .or_else(|| record.get("body").and_then(text_of))
}

/// A value that is text of at most 256 characters, or a whole number (as the number or as text).
fn clean_value(value: &Value) -> Option<Value> {
    let mut out = Map::new();
    if let Some(text) = text_of(value) {
        if text.chars().count() > MAX_TEXT {
            return None;
        }
        out.insert("stringValue".into(), Value::String(text.to_owned()));
    } else {
        let int = value.get("intValue")?;
        let fine = match int {
            Value::String(s) => {
                let digits = s.strip_prefix('-').unwrap_or(s);
                (1..=19).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
            }
            Value::Number(n) => n.is_i64() || n.is_u64(),
            _ => false,
        };
        if !fine {
            return None;
        }
        out.insert("intValue".into(), int.clone());
    }
    Some(Value::Object(out))
}

fn time_of(record: &Value) -> Option<Value> {
    let t = record.get("timeUnixNano")?;
    let fine = match t {
        Value::String(s) => (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()),
        Value::Number(n) => n.is_u64(),
        _ => false,
    };
    fine.then(|| t.clone())
}

fn rebuilt(record: &Value, name: &str) -> Value {
    let mut out = Map::new();
    out.insert("eventName".into(), Value::String(name.to_owned()));
    if let Some(t) = time_of(record) {
        out.insert("timeUnixNano".into(), t);
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut kept = Vec::new();
    for (key, value) in attributes(record) {
        if !KEYS.contains(&key) || seen.contains(&key) {
            continue; // the first of a repeated key is the one that counts
        }
        if let Some(value) = clean_value(value) {
            seen.push(key);
            let mut a = Map::new();
            a.insert("key".into(), Value::String(key.to_owned()));
            a.insert("value".into(), value);
            kept.push(Value::Object(a));
        }
    }
    out.insert("attributes".into(), Value::Array(kept));
    Value::Object(out)
}

/// Whatever `body` is, the answer is a well-formed payload (an empty one for garbage).
pub fn lenient(body: &Value) -> Filtered {
    let mut records = Vec::new();
    let (mut dropped, mut ignored, mut seen) = (0, 0, 0usize);
    let list = |v: &Value, key: &str| v.get(key).and_then(Value::as_array).cloned();
    for resource in list(body, "resourceLogs").unwrap_or_default() {
        for scope in list(&resource, "scopeLogs").unwrap_or_default() {
            for record in list(&scope, "logRecords").unwrap_or_default() {
                seen += 1;
                if seen > MAX_RECORDS {
                    ignored += 1;
                } else if let Some(name) = event_name(&record).filter(|n| EVENTS.contains(n)) {
                    records.push(rebuilt(&record, name));
                } else {
                    dropped += 1;
                }
            }
        }
    }
    let kept = records.len();
    let payload = serde_json::json!({
        "resourceLogs": [{ "scopeLogs": [{ "logRecords": records }] }]
    });
    Filtered {
        payload,
        kept,
        dropped,
        ignored,
    }
}
