//! The text of `status`: pure functions from facts to lines.
use super::messages as m;
use super::screen::{DIVIDER, Style, WIDTH, boxed, fit, width, wrap};
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

pub fn ago(at: Iso, now: Iso) -> String {
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

/// What the server said, with what this device knows next to it.
pub struct Remote<'a> {
    pub view: &'a StatusView,
    pub server: &'a str,
    pub local_fingerprint: &'a str,
    pub key_in_file: bool,
}

const LABEL: usize = 15;
const COLUMN: usize = 14;

fn row(label: &str, value: &str) -> String {
    format!(" {}{value}", pad(label, LABEL))
}

fn pad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(width(s))))
}

/// `status`: the device, one block per agent, the upload, then every warning with what to do.
/// Without `remote` (the server could not be asked) the local part alone.
pub fn status(st: Style, remote: Option<&Remote>, local: &Local, now: Iso) -> String {
    let mut lines = vec![String::new()];
    let mut warnings = Vec::new();
    if let Some(r) = remote {
        lines.extend(device_lines(st, r, now, &mut warnings));
        lines.push(DIVIDER.into());
    }
    for s in &local.sources {
        lines.extend(agent_lines(
            st,
            s,
            remote.map(|r| r.view),
            now,
            &mut warnings,
        ));
    }
    lines.push(DIVIDER.into());
    lines.extend(upload_lines(st, local, &mut warnings));
    if !warnings.is_empty() {
        lines.push(DIVIDER.into());
        for w in &warnings {
            // Off a terminal a warning stays on one line, for grep.
            let parts = if st.tty {
                wrap(w, WIDTH - 2 - 6)
            } else {
                vec![w.clone()]
            };
            for (i, part) in parts.into_iter().enumerate() {
                let lead = if i == 0 { st.warn() } else { String::new() };
                lines.push(format!(" {}{part}", pad(&lead, width(&st.warn()) + 1)));
            }
        }
    }
    lines.push(String::new());
    boxed(st, "SpendBranch status", &lines)
}

fn device_lines(st: Style, r: &Remote, now: Iso, warnings: &mut Vec<String>) -> Vec<String> {
    let view = r.view;
    let mut out = vec![row("Organization", &fit(&view.organization_name, 60))];
    if let Some(name) = &view.member_name {
        out.push(row("Member", &fit(name, 60)));
    }
    let expires = view.device.expires_at;
    out.push(row(
        "Device",
        &format!(
            "{} {} {}",
            st.ok(),
            fit(&view.device.label, 30),
            st.dim(&format!("· valid to {}", day(expires)))
        ),
    ));
    out.push(row("Server", &format!("{} {}", st.ok(), fit(r.server, 56))));
    if view.device.key_fingerprint != r.local_fingerprint {
        warnings.push(format!(
            "This device's key is {}, not the one the server has ({}). Run `spendbranch login`.",
            r.local_fingerprint, view.device.key_fingerprint
        ));
    }
    if r.key_in_file {
        warnings.push(m::KEY_IN_FILE.into());
    }
    if expires.millis() - now.millis() <= 7 * DAY_MS {
        warnings.push(format!(
            "This device expires soon ({}): run `spendbranch login` to renew it.",
            day(expires)
        ));
    }
    if view.sources.is_empty() {
        warnings.push("The company has not authorized collection yet.".into());
    }
    out
}

/// One agent: reading, hook and (with the server's answer) authorization on one row, then
/// what the server has seen; problems go to `warnings`.
fn agent_lines(
    st: Style,
    s: &LocalSource,
    view: Option<&StatusView>,
    now: Iso,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    use crate::hooks::State;
    let (name, arg) = (agent_name(s.source), s.source.arg());
    let reading = s.facts.enabled_since.is_some();
    let off = || {
        if st.color {
            st.dim("–")
        } else {
            "[-]".into()
        }
    };
    let mut cells = vec![if reading {
        format!("{} reading", st.ok())
    } else {
        format!("{} off", off())
    }];
    cells.push(match s.hook.state {
        State::Installed => format!("{} hook", st.ok()),
        State::NotInstalled => format!("{} no hook", st.warn()),
        State::Outdated => format!("{} old hook", st.warn()),
        State::Invalid => format!("{} hook file", st.bad()),
    });
    let mut detail = Vec::new();
    if let Some(view) = view {
        let granted = view.sources.iter().find(|g| g.source == s.source);
        cells.push(match granted.map(|g| g.state) {
            Some(SourceState::Active) => format!("{} authorized", st.ok()),
            Some(SourceState::Expired) => format!("{} expired", st.warn()),
            Some(SourceState::Revoked) => format!("{} revoked", st.bad()),
            Some(SourceState::Other) => format!("{} not active", st.warn()),
            None => format!("{} not authorized", st.bad()),
        });
        if reading && granted.is_none_or(|g| g.state != SourceState::Active) {
            warnings.push(format!(
                "{name} is on here but the company has not authorized it; nothing will be accepted."
            ));
        }
        if let Some(c) = view.collection.iter().find(|c| c.source == s.source) {
            let seen = c.last_seen_at.map_or("never".to_owned(), |t| ago(t, now));
            detail.push(format!("last contact {seen}"));
            if let Some(t) = c.observed_until {
                detail.push(format!("observed until {t}"));
            }
            for g in &c.open_gaps {
                warnings.push(format!(
                    "{name}: open gap {} to {} ({}): this is not zero spending.",
                    g.starts_at, g.ends_at, g.reason
                ));
            }
        }
    }
    if s.source == Source::Codex && s.hook.state == State::Installed {
        if let Some(at) = s.hook.last_firing {
            detail.push(format!("hook ran {}", ago(at, now)));
        }
        let silent = s
            .hook
            .last_firing
            .is_none_or(|at| now.millis() - at.millis() > DAY_MS);
        if reading && silent {
            warnings.push(format!(
                "{name} has not run the hook in 24 hours; it may not have trusted it: open {name} and review it under /hooks."
            ));
        }
    }
    if reading {
        match s.hook.state {
            State::NotInstalled => warnings.push(format!(
                "{name} has no hook: run `spendbranch hooks install {arg}`."
            )),
            State::Outdated => warnings.push(format!(
                "{name}'s hook is not the line this build writes (the binary moved?): run `spendbranch hooks install {arg}`."
            )),
            State::Invalid => warnings.push(format!(
                "{name}'s hook file cannot be used (not valid JSON)."
            )),
            State::Installed => {}
        }
    }
    warnings.extend(source_warnings(s, now));
    let mut out = vec![format!(
        " {}{}",
        pad(name, LABEL),
        cells
            .iter()
            .map(|c| pad(c, COLUMN))
            .collect::<String>()
            .trim_end()
    )];
    out.extend(detail.iter().map(|d| row("", &st.dim(d))));
    out
}

/// What this device could not read for an agent.
fn source_warnings(s: &LocalSource, now: Iso) -> Vec<String> {
    let name = agent_name(s.source);
    let mut out = Vec::new();
    if s.facts.unreadable_files > 0 {
        out.push(format!(
            "{name}: {} files could not be read (unknown format or no permission); they become gaps, not zero spending.",
            s.facts.unreadable_files
        ));
    }
    if let Some(v) = s
        .facts
        .untested_version
        .as_deref()
        .filter(|v| v.len() <= 32)
    {
        out.push(format!("{name} version {v} is outside the tested range."));
    }
    let stale = s
        .facts
        .last_read_at
        .as_deref()
        .and_then(Iso::parse)
        .is_none_or(|at| now.millis() - at.millis() > DAY_MS);
    if s.facts.enabled_since.is_some() && stale {
        out.push(format!(
            "{name} was last read more than 24 hours ago: run `spendbranch sync`."
        ));
    }
    out
}

/// The buffer, when it was last sent, the schedule, and what has gone wrong.
fn upload_lines(st: Style, local: &Local, warnings: &mut Vec<String>) -> Vec<String> {
    let mut waiting = format!("{} waiting", local.items);
    if local.coverage > 0 {
        waiting.push_str(&format!(" · {} coverage reports", local.coverage));
    }
    let mut out = vec![row("Upload", &waiting)];
    for (label, value) in [
        ("Last sent", &local.last_sent),
        ("Next attempt", &local.next_attempt),
    ] {
        if let Some(v) = value.as_deref().and_then(Iso::parse) {
            out.push(row(label, &v.to_string()));
        }
    }
    out.push(row(
        "Schedule",
        &if local.scheduled {
            format!("{} installed", st.ok())
        } else {
            format!("{} not installed", st.warn())
        },
    ));
    if !local.scheduled {
        warnings.push("No scheduled sync: run `spendbranch setup`.".into());
    }
    if let Some(code) = local
        .last_error
        .as_deref()
        .filter(|c| crate::api::is_plain_code(c))
    {
        warnings.push(format!("Last upload failed: {}", m::failure_message(code)));
    }
    for (code, n) in &local.rejected {
        warnings.push(format!("{}.", m::rejected_line(code, *n)));
    }
    out
}
