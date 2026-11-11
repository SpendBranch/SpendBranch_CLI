//! Every user-facing text lives here, in English. Pure: no I/O.

pub const NOT_CONNECTED: &str =
    "This device is not connected to an organization. Run `spendbranch login`.";
pub const ALREADY_DISCONNECTED: &str = "This device was already disconnected.";
pub const SYNC_RUNNING: &str = "Another `spendbranch sync` is running on this device.";
pub const NOT_AVAILABLE: &str = "This command is not available in this build yet.";

pub const USAGE_SETUP: &str =
    "Usage: spendbranch setup [--server URL] [--source claude-code|codex] [--relogin]";
pub const USAGE_ENABLE: &str = "Usage: spendbranch enable claude-code | spendbranch enable codex";
pub const USAGE_DISABLE: &str =
    "Usage: spendbranch disable claude-code | spendbranch disable codex";
pub const USAGE_HOOKS: &str = "Usage: spendbranch hooks install claude-code | spendbranch hooks uninstall claude-code | spendbranch hooks install codex | spendbranch hooks uninstall codex";
pub const USAGE_OTEL: &str = "Usage: spendbranch otel <source>. Sources: claude_code, codex.";
pub const OTEL_CODEX_REFUSED: &str = "Codex is not collected through OpenTelemetry: its telemetry lacks response IDs, so duplicate counting cannot be prevented. Use `spendbranch hooks install codex`.";

pub fn help(version: &str) -> String {
    format!(
        "spendbranch {version}

Usage:
  spendbranch setup [--server URL] [--source claude-code|codex] [--relogin]
                                     connects the device, discovered agents, and hooks at once
  spendbranch login [--server URL]   connects this device to your organization
  spendbranch status                 shows the organization, device, collection, and local buffer
  spendbranch sync                   reads installed sources, buffers data, and sends it to the server
  spendbranch enable claude-code     enables reading Claude Code transcripts
  spendbranch disable claude-code    stops reading while preserving the cursor
  spendbranch hooks install claude-code   installs the Claude Code session-end hook
  spendbranch hooks uninstall claude-code removes that hook, preserving others
  spendbranch enable codex           enables reading Codex rollouts
  spendbranch disable codex          stops reading Codex while preserving the cursor
  spendbranch hooks install codex    installs the Codex session-end hook
  spendbranch hooks uninstall codex  removes that hook, preserving others
  spendbranch otel <source>          writes the agent OTLP configuration (local relay endpoint)
  spendbranch relay [--port N]       filters agent OTLP on this machine and forwards it
  spendbranch logout                 revokes the device and deletes local credentials
  spendbranch --version

The CLI sends only company-authorized metadata (IDs, model, token counts,
normalized repository, and branch). Prompts, responses, code, and paths never leave the device."
    )
}

/// An error code from the server is shown only if it looks like one: it is text we do not
/// control, and a terminal would obey any escape sequence hidden in it.
fn is_plain_code(code: &str) -> bool {
    (1..=64).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

pub fn failure_message(code: &str) -> String {
    match code {
        "NETWORK" => "Could not reach the server. Check your network and server address (--server).".into(),
        "UNAUTHENTICATED" => "This device was revoked or expired. Run `spendbranch login`.".into(),
        "COLLECTION_NOT_AUTHORIZED" => "The company has not authorized collection. Ask the organization owner.".into(),
        "FIELDS_NOT_AUTHORIZED" => "The company authorized a different field list for this source. Ask the owner to renew authorization.".into(),
        "PLAN_REQUIRED" => "The organization has no active plan.".into(),
        "FORBIDDEN" => "Your role does not allow collection. Ask an organization admin for the developer role.".into(),
        "RATE_LIMITED" => "The server requested a pause (rate limit).".into(),
        "BEFORE_COLLECTION_WINDOW" => "Events precede the month when the company authorized collection.".into(),
        "SESSION_OWNED" => "This session already belongs to another member. Only the person who ran it can send its usage.".into(),
        "INTERNAL" => "The server failed to receive the data.".into(),
        other if is_plain_code(other) => format!("The server rejected the request ({other})."),
        _ => "The server rejected the request.".into(),
    }
}
