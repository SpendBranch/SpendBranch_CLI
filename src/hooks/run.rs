//! The rules of the hook itself: what it may log, and when it starts a `sync`. Pure.
use crate::contract::Iso;
use serde_json::{Value, json};

/// A `Stop` (or anything that is not the end of a session) starts a `sync` at most this often.
pub const INTERVAL_MS: i64 = 10 * 60 * 1000;
/// What the hook reads of the agent's stdin: it is only there to be logged.
pub const MAX_INPUT: usize = 64 * 1024;
const MAX_FIELD: usize = 64;

/// Letters, digits and `-_.:` only, at most 64: what gets into `hook.log`. Anything else is
/// not logged, so nothing the agent says can end up there.
pub fn plain(text: &str) -> Option<String> {
    let ok = !text.is_empty()
        && text.len() <= MAX_FIELD
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c));
    ok.then(|| text.to_owned())
}

/// The event and the session out of Claude's stdin JSON. The rest of it (the transcript path,
/// the prompt) is never looked at.
pub fn parse_input(bytes: &[u8]) -> (Option<String>, Option<String>) {
    let Ok(Value::Object(map)) =
        serde_json::from_slice::<Value>(&bytes[..bytes.len().min(MAX_INPUT)])
    else {
        return (None, None);
    };
    let field = |k: &str| map.get(k).and_then(Value::as_str).and_then(plain);
    (field("hook_event_name"), field("session_id"))
}

/// A line of `hook.log`.
pub fn log_line(at: Iso, event: &str, session: Option<&str>) -> String {
    let mut line = json!({ "at": at.to_string(), "event": event });
    if let (Some(s), Some(map)) = (session, line.as_object_mut()) {
        map.insert("session".into(), json!(s));
    }
    line.to_string()
}

/// The last time the log says `event_prefix` ran (a Codex line has `"event":"codex:...`).
pub fn last_firing(log: &str, event_prefix: &str) -> Option<Iso> {
    log.lines().rev().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        value
            .get("event")?
            .as_str()?
            .starts_with(event_prefix)
            .then(|| value.get("at")?.as_str().and_then(Iso::parse))?
    })
}

/// Whether the hook starts a `sync`. The end of a session always does; anything else waits
/// for the interval since the last one. A last sync in the future (the clock went back) does
/// not block forever.
pub fn due(event: Option<&str>, last_sync: Option<Iso>, now: Iso) -> bool {
    if event == Some("SessionEnd") {
        return true;
    }
    last_sync.is_none_or(|last| !(0..INTERVAL_MS).contains(&(now.millis() - last.millis())))
}
