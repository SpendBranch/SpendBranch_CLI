//! What goes into the agents' hook files, and the rules of the hook itself. Pure: text in,
//! text out. The files are read and written in `commands`.
//!
//! The edit keeps everything that is not ours: other hooks, other settings, the order of
//! the keys. Only an entry that `is_ours` is ever replaced or removed.
use crate::contract::{Iso, Source};
use serde_json::{Map, Value, json};

/// A `Stop` (or anything that is not the end of a session) starts a `sync` at most this often.
pub const INTERVAL_MS: i64 = 10 * 60 * 1000;
/// What the hook reads of the agent's stdin: it is only there to be logged.
pub const MAX_INPUT: usize = 64 * 1024;
const MAX_FIELD: usize = 64;

/// Why a file was left alone.
#[derive(Debug, PartialEq, Eq)]
pub enum Problem {
    NotJson,
    /// Valid JSON, but `hooks` or an event is not the shape the agent documents.
    Layout,
}

/// What one hook file holds about us.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum State {
    NotInstalled,
    Installed,
    /// Ours is there but not as this build would write it: another binary, a missing event,
    /// or the line the TypeScript CLI wrote.
    Outdated,
    Invalid,
}

/// The result of an edit: what to write, or that the file is already as wanted.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    Unchanged,
    Write(String),
}

/// The entries we own: (event, runs in the background, timeout in seconds, arguments).
fn wanted(source: Source) -> [(&'static str, bool, u64, &'static str); 2] {
    match source {
        Source::ClaudeCode => [
            ("SessionEnd", true, 10, "hook claude-code"),
            ("Stop", true, 10, "hook claude-code"),
        ],
        Source::Codex => [
            ("SessionEnd", false, 3, "hook codex SessionEnd"),
            ("Stop", true, 30, "hook codex Stop"),
        ],
    }
}

/// The command line for the executable and its arguments: quoted so a path with spaces works
/// in the shell each agent uses. Windows uses forward slashes, which Git Bash, PowerShell and
/// `cmd` read the same way.
pub fn command(exe: &str, windows: bool, args: &str) -> String {
    if windows {
        format!("\"{}\" {args}", exe.replace('\\', "/"))
    } else {
        format!("'{}' {args}", exe.replace('\'', "'\\''"))
    }
}

/// Our entry, including the one the TypeScript CLI wrote (`node '<script>' hook claude-code`).
pub fn is_ours(source: Source, command: &str) -> bool {
    let command = command.trim_end();
    command.contains("spendbranch")
        && match source {
            Source::ClaudeCode => command.ends_with(" hook claude-code"),
            Source::Codex => command.contains(" hook codex "),
        }
}

fn ours(source: Source, entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| is_ours(source, c))
}

fn parse(original: Option<&str>) -> Result<Value, Problem> {
    let Some(text) = original else {
        return Ok(Value::Object(Map::new()));
    };
    match serde_json::from_str::<Value>(text) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(Problem::NotJson),
    }
}

fn finish(before: &Value, after: Value, existed: bool) -> Plan {
    if existed && *before == after {
        return Plan::Unchanged;
    }
    // Two spaces and a final newline: what the agents write themselves.
    let mut text = serde_json::to_string_pretty(&after).unwrap_or_default();
    text.push('\n');
    Plan::Write(text)
}

/// Removes our entries from the groups of one event. With `keep`, the first one found is
/// replaced by it instead (and the rest go). Groups we emptied are dropped; others stay.
fn strip(source: Source, groups: &mut Vec<Value>, mut keep: Option<&Value>) -> bool {
    let mut placed = false;
    groups.retain_mut(|group| {
        let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = list.len();
        list.retain_mut(|entry| {
            if !ours(source, entry) {
                return true;
            }
            match keep.take() {
                Some(new) => {
                    *entry = new.clone();
                    placed = true;
                    true
                }
                None => false,
            }
        });
        !(list.is_empty() && before > 0)
    });
    placed
}

/// Puts our entries in (replacing an older or foreign-binary line of ours). `exe` is the
/// executable's absolute path.
pub fn install(
    source: Source,
    original: Option<&str>,
    exe: &str,
    windows: bool,
) -> Result<Plan, Problem> {
    let before = parse(original)?;
    let mut root = before.clone();
    let top = root.as_object_mut().ok_or(Problem::NotJson)?;
    let hooks = top
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or(Problem::Layout)?;
    for (event, background, timeout, args) in wanted(source) {
        let mut entry = Map::new();
        entry.insert("type".into(), json!("command"));
        entry.insert("command".into(), json!(command(exe, windows, args)));
        if background {
            entry.insert("async".into(), json!(true));
        }
        entry.insert("timeout".into(), json!(timeout));
        let entry = Value::Object(entry);
        let groups = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or(Problem::Layout)?;
        if !strip(source, groups, Some(&entry)) {
            groups.push(json!({ "hooks": [entry] }));
        }
    }
    Ok(finish(&before, root, original.is_some()))
}

/// Takes our entries out and nothing else. Events, `hooks` and groups we emptied go too.
pub fn uninstall(source: Source, original: Option<&str>) -> Result<Plan, Problem> {
    let before = parse(original)?;
    if original.is_none() {
        return Ok(Plan::Unchanged); // no file: nothing of ours, and no file to create
    }
    let mut root = before.clone();
    let top = root.as_object_mut().ok_or(Problem::NotJson)?;
    if let Some(hooks) = top.get_mut("hooks") {
        let hooks = hooks.as_object_mut().ok_or(Problem::Layout)?;
        let had = !hooks.is_empty();
        let mut emptied = Vec::new();
        for (event, groups) in hooks.iter_mut() {
            let Some(groups) = groups.as_array_mut() else {
                continue;
            };
            let had = !groups.is_empty();
            strip(source, groups, None);
            if had && groups.is_empty() {
                emptied.push(event.clone());
            }
        }
        for event in emptied {
            hooks.shift_remove(&event);
        }
        // Only when it held something and we emptied it: an empty `hooks` the user wrote stays.
        if had && hooks.is_empty() {
            top.shift_remove("hooks");
        }
    }
    Ok(finish(&before, root, original.is_some()))
}

/// How the file stands for this binary.
pub fn state(source: Source, original: Option<&str>, exe: &str, windows: bool) -> State {
    match install(source, original, exe, windows) {
        Err(_) => State::Invalid,
        Ok(Plan::Unchanged) => State::Installed,
        Ok(Plan::Write(_)) => match uninstall(source, original) {
            Ok(Plan::Write(_)) => State::Outdated,
            _ => State::NotInstalled,
        },
    }
}

// --- the hook itself ------------------------------------------------------------------

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
