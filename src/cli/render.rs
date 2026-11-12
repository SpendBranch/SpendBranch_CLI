//! The text of `status`: pure functions from facts to lines.
use crate::api::{SourceState, StatusView};
use crate::contract::{Iso, Source};
use crate::sources::outcome::SourceFacts;

pub fn agent_name(source: Source) -> &'static str {
    match source {
        Source::ClaudeCode => "Claude Code",
        Source::Codex => "Codex",
    }
}

const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// `2027-02-08` from a timestamp.
fn day(at: Iso) -> String {
    at.to_string().chars().take(10).collect()
}

fn ago(at: Iso, now: Iso) -> String {
    let hours = (now.millis() - at.millis()) / HOUR_MS;
    match hours {
        h if h >= 48 => format!("{} days ago", h / 24),
        h if h >= 1 => format!("{h} h ago"),
        _ => "less than an hour ago".into(),
    }
}

/// How the agent's hook stands, and (Codex) when it last ran.
pub struct HookView {
    pub state: crate::hooks::State,
    pub last_firing: Option<Iso>,
}

pub struct LocalSource {
    pub source: Source,
    pub facts: SourceFacts,
    pub hook: HookView,
    /// The folder that is read: shown here, never sent.
    pub folder: String,
}

pub struct Local {
    pub items: u64,
    pub coverage: u64,
    pub rejected: Vec<(String, u64)>,
    pub last_sent: Option<String>,
    pub last_error: Option<String>,
    pub next_attempt: Option<String>,
    pub sources: Vec<LocalSource>,
    /// The system runs `sync` on a schedule.
    pub scheduled: bool,
}

/// The part that needs no network.
pub fn local_lines(local: &Local, now: Iso) -> Vec<String> {
    let mut out = Vec::new();
    let mut buffer = format!("Local buffer: {} items awaiting upload", local.items);
    if local.coverage > 0 {
        buffer.push_str(&format!(" and {} coverage reports", local.coverage));
    }
    out.push(buffer);
    for (label, value) in [
        ("Last sent", &local.last_sent),
        ("Next attempt", &local.next_attempt),
    ] {
        if let Some(v) = value.as_deref().and_then(Iso::parse) {
            out.push(format!("  {label}: {v}"));
        }
    }
    if let Some(code) = local
        .last_error
        .as_deref()
        .filter(|c| crate::api::is_plain_code(c))
    {
        out.push(format!(
            "  Last error: {}",
            super::messages::failure_message(code)
        ));
    }
    if !local.rejected.is_empty() {
        let list: Vec<String> = local
            .rejected
            .iter()
            .map(|(c, n)| format!("{c}: {n}"))
            .collect();
        out.push(format!("  Refused or dropped: {}", list.join(", ")));
    }
    for s in &local.sources {
        let name = agent_name(s.source);
        match s.facts.enabled_since.as_deref().and_then(Iso::parse) {
            Some(since) => out.push(format!(
                "{name}: collection on since {} (reading {}, {} files tracked)",
                day(since),
                s.folder,
                s.facts.tracked_files
            )),
            None => out.push(format!("{name}: collection off here")),
        }
        out.extend(hook_lines(s, now));
        if s.facts.unreadable_files > 0 {
            out.push(format!(
                "  unread files (unrecognized format or no permission): {} (become gaps, not zero spending)",
                s.facts.unreadable_files
            ));
        }
        if let Some(v) = s
            .facts
            .untested_version
            .as_deref()
            .filter(|v| v.len() <= 32)
        {
            out.push(format!("  {name} version outside the tested range: {v}"));
        }
        let stale = s
            .facts
            .last_read_at
            .as_deref()
            .and_then(Iso::parse)
            .is_none_or(|at| now.millis() - at.millis() > DAY_MS);
        if s.facts.enabled_since.is_some() && stale {
            out.push("  last read was more than 24 hours ago: run `spendbranch sync`".into());
        }
    }
    out.push(if local.scheduled {
        "Scheduled sync: installed".into()
    } else {
        "Scheduled sync: not installed (run `spendbranch setup`)".into()
    });
    out
}

fn hook_lines(s: &LocalSource, now: Iso) -> Vec<String> {
    use crate::hooks::State;
    let (name, arg) = match s.source {
        Source::ClaudeCode => ("Claude Code", "claude-code"),
        Source::Codex => ("Codex", "codex"),
    };
    let mut out = vec![match s.hook.state {
        State::NotInstalled => {
            format!("  Hook: not installed (run `spendbranch hooks install {arg}`)")
        }
        State::Installed => "  Hook: installed".into(),
        State::Outdated => format!(
            "  Hook: installed, but it is not the line this build writes (the binary moved?): run `spendbranch hooks install {arg}`"
        ),
        State::Invalid => "  Hook: the agent's hook file cannot be used (not valid JSON)".into(),
    }];
    if s.source == Source::Codex && s.hook.state == State::Installed {
        if let Some(at) = s.hook.last_firing {
            out.push(format!("  Last hook run: {at}"));
        }
        let silent = s
            .hook
            .last_firing
            .is_none_or(|at| now.millis() - at.millis() > DAY_MS);
        if s.facts.enabled_since.is_some() && silent {
            out.push(format!(
                "  {name} has not run the hook in 24 hours; it may not have trusted it: open {name} and review it under /hooks"
            ));
        }
    }
    out
}

/// The part the server told us, with what this device knows next to it.
pub fn remote_lines(
    view: &StatusView,
    server: &str,
    local_fingerprint: &str,
    key_in_file: bool,
    local: &Local,
    now: Iso,
) -> Vec<String> {
    let mut out = vec![format!("Organization: {}", view.organization_name)];
    if let Some(name) = &view.member_name {
        out.push(format!("Member: {name}"));
    }
    out.push(format!(
        "Device: {} ({})",
        view.device.label, view.device.key_fingerprint
    ));
    if view.device.key_fingerprint != local_fingerprint {
        out.push(format!(
            "  WARNING: this device's key is {local_fingerprint}, not what the server has. Run `spendbranch login`."
        ));
    }
    if key_in_file {
        out.push(
            "  WARNING: no system keychain; the key is in a file in the data folder (less safe)"
                .into(),
        );
    }
    out.push(format!("Server: {server} (reachable)"));
    let left = view.device.expires_at.millis() - now.millis();
    let mut valid = format!("Device valid until: {}", day(view.device.expires_at));
    if left <= 7 * DAY_MS {
        valid.push_str(" (expires soon: run `spendbranch login` to renew)");
    }
    out.push(valid);
    out.push("Authorized sources:".into());
    for s in &view.sources {
        let name = agent_name(s.source);
        out.push(match (s.state, s.window_start) {
            (SourceState::Active, Some(w)) => format!(
                "  {name}: active since {} ({})",
                day(w),
                s.parser_version.as_str()
            ),
            (SourceState::Active, None) => {
                format!("  {name}: active ({})", s.parser_version.as_str())
            }
            (SourceState::Expired, _) => format!("  {name}: authorization expired"),
            (SourceState::Revoked, _) => format!("  {name}: authorization revoked"),
            (SourceState::Other, _) => format!("  {name}: not active"),
        });
    }
    if view.sources.is_empty() {
        out.push("  none: the company has not authorized collection yet".into());
    }
    for l in &local.sources {
        let active = view
            .sources
            .iter()
            .any(|s| s.source == l.source && s.state == SourceState::Active);
        if l.facts.enabled_since.is_some() && !active {
            out.push(format!(
                "  WARNING: {} is on here but the company has not authorized it; nothing will be accepted",
                agent_name(l.source)
            ));
        }
    }
    if !view.collection.is_empty() {
        out.push("Collection (what the server has received):".into());
    }
    for c in &view.collection {
        let name = agent_name(c.source);
        let seen = c.last_seen_at.map_or("never".to_owned(), |t| ago(t, now));
        let until = c.observed_until.map_or("nothing yet".to_owned(), |t| {
            format!("{} ({})", t, ago(t, now))
        });
        out.push(format!(
            "  {name}: last contact {seen}, observed until {until}"
        ));
        for g in &c.open_gaps {
            out.push(format!(
                "    open gap {} to {} ({}): this is not zero spending",
                g.starts_at, g.ends_at, g.reason
            ));
        }
    }
    out
}
