//! What goes into the agents' hook files. Pure: text in, text out. The files are read and
//! written in `commands`.
//!
//! The edit keeps everything that is not ours: other hooks, other settings, the order of
//! the keys. Only an entry that `is_ours` is ever replaced or removed.
use crate::contract::Source;
use serde_json::{Map, Value, json};

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

/// The words of a command line as `command` writes it: `'...'` and `"..."` quote, `\` outside
/// quotes takes the next character as it is (so `'\''` is a quote inside a quoted word), and
/// whitespace separates. `None` for an unterminated quote.
fn words(command: &str) -> Option<Vec<String>> {
    let (mut out, mut word, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '\\') => {
                word.push(chars.next()?);
                started = true;
            }
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => (quote, started) = (Some(c), true),
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut word));
                }
                started = false;
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        out.push(word);
    }
    Some(out)
}

/// Our entry: a program whose file name starts with `spendbranch` (any folder, any suffix such
/// as `spendbranch-x86_64-linux`: another install of ours is replaced) followed by exactly our
/// arguments, or the line the TypeScript CLI wrote (`node '<.../spendbranch/...>' hook
/// claude-code`). A command that only mentions `spendbranch` somewhere is someone else's.
/// `install` also owns the exact line it writes, whatever the binary is called.
pub fn is_ours(source: Source, command: &str) -> bool {
    let Some(words) = words(command) else {
        return false;
    };
    let name = |p: &str| {
        p.rsplit(['/', '\\'])
            .next()
            .unwrap_or(p)
            .to_ascii_lowercase()
    };
    let args = match words.as_slice() {
        [program, args @ ..] if name(program).starts_with("spendbranch") => args,
        [node, script, args @ ..]
            if matches!(name(node).as_str(), "node" | "node.exe")
                && script.contains("spendbranch") =>
        {
            args
        }
        _ => return false,
    };
    match source {
        Source::ClaudeCode => args == ["hook", "claude-code"],
        Source::Codex => matches!(args, [h, c, _] if h == "hook" && c == "codex"),
    }
}

/// Ours by its shape, or the very line `keep` holds (a binary under any name).
fn ours(source: Source, entry: &Value, keep: Option<&Value>) -> bool {
    let command = |e: &Value| e.get("command").and_then(Value::as_str).map(str::to_owned);
    let line = command(entry);
    line.as_deref().is_some_and(|c| is_ours(source, c))
        || (line.is_some() && keep.and_then(command) == line)
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
    // `{:#}` is the pretty form and, unlike `to_string_pretty`, cannot fail: a failure here
    // would have written an empty file over the user's settings.
    let mut text = format!("{after:#}");
    text.push('\n');
    Plan::Write(text)
}

/// Removes our entries from the groups of one event. With `keep`, the first one found is
/// replaced by it instead (and the rest go). Groups we emptied are dropped; others stay.
fn strip(source: Source, groups: &mut Vec<Value>, mut keep: Option<&Value>) -> bool {
    let wanted = keep;
    let mut placed = false;
    groups.retain_mut(|group| {
        let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = list.len();
        list.retain_mut(|entry| {
            if !ours(source, entry, wanted) {
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
