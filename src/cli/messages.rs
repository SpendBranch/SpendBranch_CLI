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

use crate::api::{ApiFailure, is_plain_code};

pub fn failure_message(code: &str) -> String {
    match code {
        "NETWORK" => "Could not reach the server. Check your network and server address (--server).".into(),
        "UNAUTHENTICATED" => "This device was revoked or expired. Run `spendbranch login`.".into(),
        "COLLECTION_NOT_AUTHORIZED" => "The company has not authorized collection. Ask the organization owner.".into(),
        "FIELDS_NOT_AUTHORIZED" => "The company authorized a different field list for this source. Ask the owner to renew authorization.".into(),
        "PLAN_REQUIRED" => "The organization has no active plan.".into(),
        "FORBIDDEN" => "Your role does not allow collection. Ask an organization admin for the developer role.".into(),
        "RATE_LIMITED" => "The server requested a pause (rate limit).".into(),
        "CLOCK_SKEW" => "Your computer's clock is out of step with the server's. Fix the date and time, then try again.".into(),
        "BEFORE_COLLECTION_WINDOW" => "Events precede the month when the company authorized collection.".into(),
        "SESSION_OWNED" => "This session already belongs to another member. Only the person who ran it can send its usage.".into(),
        "INTERNAL" => "The server failed to receive the data.".into(),
        other if is_plain_code(other) => format!("The server rejected the request ({other})."),
        _ => "The server rejected the request.".into(),
    }
}

/// The text for a failed call. A clock problem says how far off the clock is, when the server told us.
pub fn failure_text(failure: &ApiFailure, local_now_ms: i64) -> String {
    match (failure.code.as_str(), failure.server_time_ms) {
        ("CLOCK_SKEW", Some(server)) => {
            let minutes = (local_now_ms - server) / 60_000;
            let side = if minutes >= 0 { "ahead of" } else { "behind" };
            format!(
                "Your computer's clock is about {} minutes {side} the server's. Fix the date and time, then try again.",
                minutes.abs()
            )
        }
        _ => failure_message(&failure.code),
    }
}

pub const BAD_STATUS: &str = "The server's status answer was not valid.";
pub const KEY_MISSING: &str = "The device key is missing or unreadable. Run `spendbranch login` to connect this device again.";
pub const KEY_IN_FILE: &str = "No system keychain was found: the device key is kept in a file in the data folder (less safe).";
pub const LOGIN_BAD_PAIRING: &str = "The server's pairing answer was not valid (for example, a verification address on another domain); sign-in was stopped.";
pub const LOGIN_DENIED: &str = "Sign-in was denied in the browser, or your role does not allow collection, or the company has not authorized collection.";
pub const LOGIN_EXPIRED: &str = "The code expired before approval. Run `spendbranch login` again. If the browser refused the approval, the company may not have authorized collection yet.";
pub const LOGIN_CANCELLED: &str = "Cancelled. This device was not connected.";
pub const LOGIN_LOCK_BUSY: &str = "The new access was approved, but another `spendbranch sync` kept running and it was not saved. Run `spendbranch login` again after the sync ends.";
pub const OLD_DEVICE_NOT_REVOKED: &str = "The previous device could not be revoked on the server. Revoke it in \"My devices\" on the web.";
pub const BUFFER_DISCARDED: &str = "This is a different organization: the buffer from the previous one was discarded, and collection was turned off. Run `spendbranch enable <source>` again.";
pub const LOGOUT_REVOKED: &str = "Device revoked on the server.";
pub const LOGOUT_ALREADY_REVOKED: &str = "The device was already revoked or expired.";
pub const LOGOUT_NOT_REVOKED: &str =
    "The device could not be revoked on the server. Revoke it in \"My devices\" on the web.";
pub const LOGOUT_LOCAL_DELETED: &str = "Local credentials deleted.";
pub const DISABLED: &str = "Collection turned off. The reading position was kept.";
pub const NOT_AUTHORIZED_PREFIX: &str = "The company has not authorized";
pub const INVALID_SERVER: &str = "Invalid server address";

pub fn ready(organization: &str) -> String {
    format!("Ready: this device collects for {organization}.")
}

pub fn confirm_organization(organization: &str) -> String {
    format!(
        "This device will send usage metadata to \"{organization}\". Is that your organization? [y/N] "
    )
}

pub fn not_authorized(agent: &str) -> String {
    format!("The company has not authorized {agent} collection. Ask the organization owner.")
}

pub fn different_fields(agent: &str) -> String {
    format!(
        "The company authorized a different {agent} field list. Ask the owner to renew authorization."
    )
}

pub const NO_SOURCE_ON: &str =
    "No source is turned on. Run `spendbranch enable claude-code` or `spendbranch enable codex`.";
pub const SESSION_OWNED_STOP: &str = "A session in the buffer belongs to another member, so it was discarded and the upload stopped. If the server disconnected this device, `spendbranch status` will say so: run `spendbranch login` to connect again.";

/// One line of the summary for a code and how many events it covers.
pub fn rejected_line(code: &str, n: u64) -> String {
    match code {
        "CLOCK_SKEW_EXPIRED" => format!("{n} items dropped: their time was ahead of the server"),
        "INVALID_LOCAL" => format!("{n} events dropped: they did not pass the local check"),
        "VALIDATION_FAILED" => {
            format!("{n} events dropped: the server said the batch was out of contract")
        }
        "BEFORE_COLLECTION_WINDOW" => format!(
            "{n} events dropped: they are from before the month the company authorized collection"
        ),
        other => format!(
            "{n} events rejected by the server: {}",
            failure_message(other)
        ),
    }
}

// --- hooks, scheduler, setup ----------------------------------------------------------

pub const NO_HOME: &str = "Cannot locate your home folder.";
pub const CODEX_TRUST: &str = "Open Codex and trust the hook under /hooks; Codex ignores new or changed hooks until they are reviewed.";
pub const HOME_NOT_CARRIED: &str = "Note: SPENDBRANCH_HOME is set here, but the agent starts the hook with its own environment. Set it there too if you use a custom data folder.";
pub const SETUP_NO_AGENT: &str = "No supported agent was found (Claude Code or Codex). Install one, then run `spendbranch setup` again.";
pub const SETUP_RECONNECT: &str = "This device was revoked or expired; reconnecting.";
pub const SCHEDULER_REMOVED: &str = "Scheduled sync removed.";

pub fn hook_file_problem(file: &str, layout: bool) -> String {
    if layout {
        format!(
            "File {file} does not have the layout the agent documents for hooks; nothing was changed."
        )
    } else {
        format!("File {file} is not valid JSON; nothing was changed.")
    }
}

pub fn hook_installed(agent: &str, file: &str) -> String {
    format!("Installed the {agent} hook in {file}.")
}

pub fn hook_already(agent: &str) -> String {
    format!("The {agent} hook is already installed.")
}

pub fn hook_removed(agent: &str, file: &str) -> String {
    format!("Removed the {agent} hook from {file}.")
}

pub fn hook_absent(agent: &str) -> String {
    format!("The {agent} hook was not installed.")
}

pub fn hook_hint(agent: &str, arg: &str) -> String {
    format!(
        "The {agent} hook is not installed: run `spendbranch hooks install {arg}` so the usage is sent when a session ends."
    )
}

pub fn setup_server_differs(saved: &str, wanted: &str) -> String {
    format!(
        "This device is connected to {saved}. To switch to {wanted}, run `spendbranch setup --relogin --server {wanted}`."
    )
}

pub fn setup_already(organization: &str) -> String {
    format!("Already connected to {organization}. Use --relogin to switch.")
}

pub fn agent_found(agent: &str, found: bool) -> String {
    format!("{agent}: {}", if found { "found" } else { "not found" })
}

pub fn scheduler_installed(method: &str) -> String {
    format!("Scheduled sync: every 30 minutes and at login ({method}).")
}

pub fn scheduler_failed(why: &str) -> String {
    format!("Scheduled sync: not installed ({why}). Hooks still send usage when a session ends.")
}

pub fn scheduler_not_removed(why: &str) -> String {
    format!("Could not remove the scheduled sync: {why}")
}

// --- otel and relay -------------------------------------------------------------------

pub const OTEL_WARNING: &str = "Do not turn on OTEL_LOG_USER_PROMPTS, OTEL_LOG_TOOL_DETAILS or OTEL_LOG_RAW_API_BODIES. OpenTelemetry carries no history: `spendbranch sync` stays the source of the final numbers.";
pub const RELAY_DEFAULT_PORT: u16 = 4318;

pub fn otel_written(file: &str, windows: bool) -> String {
    let load = if windows {
        format!(
            "Get-Content -Raw '{}' | Invoke-Expression",
            file.replace('\'', "''")
        )
    } else {
        format!("source '{}'", file.replace('\'', "'\\''"))
    };
    format!(
        "Wrote {file}.\nLoad it in the terminal where you start Claude Code:\n  {load}\nand keep `spendbranch relay` running while you work. Nothing secret is in that file."
    )
}

pub fn relay_listening(port: u16, server: &str) -> String {
    let mut text = format!(
        "Relay listening on http://127.0.0.1:{port}{} and forwarding to {server}. Press Ctrl+C to stop.",
        crate::relay::PATH
    );
    if port != RELAY_DEFAULT_PORT {
        text.push_str(&format!(
            "\n`spendbranch otel` writes port {RELAY_DEFAULT_PORT}: point the agent's OTEL_EXPORTER_OTLP_LOGS_ENDPOINT at this port."
        ));
    }
    text
}

pub fn relay_port_busy(port: u16, why: &str) -> String {
    format!("Cannot listen on 127.0.0.1:{port}: {why}. Pick another port with `--port`.")
}

pub fn relay_dropped(n: u64, why: &str) -> String {
    format!("Dropped {n} batch(es) so far: {why}")
}
