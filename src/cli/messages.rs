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

// --- help and the start screen ------------------------------------------------------

pub const TAGLINE: &str = "· usage metadata for your organization";
pub const PRIVACY: [&str; 3] = [
    "Sends only company-authorized metadata: IDs, model, token counts,",
    "normalized repository, and branch. Prompts, responses, code, and paths",
    "never leave the device.",
];

/// The grouped command list: (group, note, [(command, what it does)]). Order = order of use.
pub type Group = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);
pub const HELP_GROUPS: [Group; 4] = [
    (
        "GET STARTED",
        "",
        &[
            ("setup", "connect this device, find agents, install hooks"),
            ("status", "what is connected and what is waiting"),
        ],
    ),
    (
        "AGENTS",
        "<agent> = claude-code | codex",
        &[
            ("enable <agent>", "start reading this agent's sessions"),
            ("disable <agent>", "stop reading (keeps your place)"),
            ("hooks install <agent>", "send usage when a session ends"),
            (
                "hooks uninstall <agent>",
                "remove that hook, others untouched",
            ),
        ],
    ),
    (
        "ACCOUNT",
        "",
        &[
            ("login [--server URL]", "connect to your organization"),
            ("logout", "revoke this device, delete credentials"),
            ("uninstall [--yes]", "remove SpendBranch from this computer"),
        ],
    ),
    (
        "ADVANCED",
        "",
        &[
            ("sync", "read and upload now (hooks do this)"),
            ("otel <agent>", "write the agent's OpenTelemetry settings"),
            (
                "relay [--port N]",
                "filter OpenTelemetry locally and forward it",
            ),
        ],
    ),
];

/// `spendbranch <name> --help`: what follows the name, what it does, its options.
pub struct CommandHelp {
    pub name: &'static str,
    pub usage: &'static str,
    pub about: &'static [&'static str],
    pub options: &'static [(&'static str, &'static str)],
}

const AGENT_ARG: (&str, &str) = ("<agent>", "claude-code | codex");

pub const COMMAND_HELP: [CommandHelp; 11] = [
    CommandHelp {
        name: "setup",
        usage: "[--server URL] [--source AGENT] [--relogin]",
        about: &[
            "Connects this device, finds Claude Code and Codex, turns on reading",
            "and installs their hooks, then schedules a sync. Safe to run again.",
        ],
        options: &[
            (
                "--server URL",
                "the SpendBranch server (or SPENDBRANCH_SERVER)",
            ),
            (
                "--source AGENT",
                "only set up this agent (claude-code | codex)",
            ),
            ("--relogin", "connect again, even if already connected"),
        ],
    },
    CommandHelp {
        name: "login",
        usage: "[--server URL]",
        about: &[
            "Connects this device to your organization: you approve a code in the",
            "browser, and the device key is kept in the system keychain.",
        ],
        options: &[(
            "--server URL",
            "the SpendBranch server (or SPENDBRANCH_SERVER)",
        )],
    },
    CommandHelp {
        name: "status",
        usage: "",
        about: &[
            "Shows the organization, this device, each agent and what is waiting",
            "to upload. The local part shows even when the server is unreachable.",
        ],
        options: &[],
    },
    CommandHelp {
        name: "sync",
        usage: "",
        about: &[
            "Reads the agents that are turned on and uploads their usage now.",
            "Hooks and the schedule run this for you.",
        ],
        options: &[],
    },
    CommandHelp {
        name: "enable",
        usage: "<agent>",
        about: &["Starts reading this agent's sessions on this device."],
        options: &[AGENT_ARG],
    },
    CommandHelp {
        name: "disable",
        usage: "<agent>",
        about: &[
            "Stops reading this agent. The reading position is kept, so `enable`",
            "picks up where it stopped.",
        ],
        options: &[AGENT_ARG],
    },
    CommandHelp {
        name: "hooks",
        usage: "install|uninstall <agent>",
        about: &[
            "Adds or removes the session-end hook in the agent's own settings,",
            "leaving its other hooks untouched.",
        ],
        options: &[AGENT_ARG],
    },
    CommandHelp {
        name: "otel",
        usage: "<agent>",
        about: &[
            "Writes the OpenTelemetry settings that point Claude Code at the local",
            "relay. Only claude-code: Codex is collected through its hook.",
        ],
        options: &[AGENT_ARG],
    },
    CommandHelp {
        name: "relay",
        usage: "[--port N]",
        about: &[
            "Receives the agent's OpenTelemetry on this machine, keeps only the",
            "authorized fields and forwards them. Runs until Ctrl+C.",
        ],
        options: &[("--port N", "listen here (default: the port `otel` chose)")],
    },
    CommandHelp {
        name: "logout",
        usage: "",
        about: &["Revokes this device on the server and deletes its local credentials."],
        options: &[],
    },
    CommandHelp {
        name: "uninstall",
        usage: "[--yes]",
        about: &[
            "Removes SpendBranch from this computer: disconnects the device, removes",
            "the agent hooks, the scheduled sync, the PATH lines the installer added,",
            "the data folder and the program. Other hooks and settings stay.",
        ],
        options: &[("--yes", "do not ask first (for scripts)")],
    },
];

pub fn command_help(name: &str) -> Option<&'static CommandHelp> {
    COMMAND_HELP.iter().find(|h| h.name == name)
}

pub fn unknown_command(name: &str) -> String {
    format!("unknown command '{name}'")
}

pub const HOME_NOT_CONNECTED: &str = "Not connected";
pub const HOME_START: &str = "Start here";
pub const HOME_SETUP_WHAT: &str = "connects, finds agents, adds hooks";
pub const HOME_PRIVACY: [&str; 3] = [
    "Privacy",
    "Only company-authorized metadata.",
    "Prompts and code never leave.",
];
pub const HOME_CONNECTED: &str = "this device is connected";
pub const HOME_UNREADABLE: &str = "Could not read the local state";

use crate::api::{ApiFailure, is_plain_code};

pub fn failure_message(code: &str) -> String {
    match code {
        "NETWORK" => "Could not reach the server. Check your network and server address (--server).".into(),
        "UNAUTHENTICATED" => "This device was revoked, expired, or lost access. Run `spendbranch login`.".into(),
        "COLLECTION_NOT_AUTHORIZED" => "The company has not authorized collection. Ask the organization owner.".into(),
        "FIELDS_NOT_AUTHORIZED" => "The company authorized a different field list for this source. Ask the owner to renew authorization.".into(),
        "PLAN_REQUIRED" => "The organization has no active plan.".into(),
        "FORBIDDEN" => "Your role does not allow collection. Ask an organization admin for the developer role.".into(),
        "RATE_LIMITED" => "The server requested a pause (rate limit).".into(),
        "CLOCK_SKEW" => "Your computer's clock is out of step with the server's. Fix the date and time, then try again.".into(),
        "BEFORE_COLLECTION_WINDOW" => "Events precede the month when the company authorized collection.".into(),
        "SESSION_OWNED" => "This session already belongs to another member. Only the person who ran it can send its usage.".into(),
        "INTERNAL" => "The server failed to receive the data.".into(),
        "VALIDATION_FAILED" => "The server refused the data as out of contract. Install the latest spendbranch; if it keeps happening, report it.".into(),
        "PAYLOAD_TOO_LARGE" => "The data sent was too large for the server.".into(),
        "UNSUPPORTED_MEDIA_TYPE" => "The server did not accept the upload format. A proxy in between may be changing requests; check the server address (--server).".into(),
        "UNAVAILABLE" => "The server is temporarily unavailable. Try again later.".into(),
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
pub const LOGOUT_ALREADY_REVOKED: &str =
    "The device was already revoked, expired, or without access.";
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
        "This device will send usage metadata to \"{organization}\".\nIs that your organization? [y/N] "
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
pub const BAD_INSTALLATION_ID: &str = "The installation id in the local state is not valid, so nothing can be sent. The buffer was kept. Run `spendbranch uninstall` and set up again, or report it.";
pub const SESSION_OWNED_STOP: &str = "A session in the buffer belongs to another member, so it was discarded and the upload stopped. Run `spendbranch status` to check that this device is still connected.";

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
pub const SETUP_RECONNECT: &str = "This device was revoked, expired, or lost access; reconnecting.";
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

/// The fields on one line: how many, and where the whole list is.
pub fn fields_line(agent: &str, arg: &str, fields: &[&str]) -> String {
    format!(
        "Reads {} fields from {agent} sessions, all on the company's list (`spendbranch enable {arg}` names them).",
        fields.len()
    )
}

pub fn schedule_every(method: &str) -> String {
    format!(
        "every {} minutes and at login ({method})",
        crate::scheduler::INTERVAL_MIN
    )
}

pub const SETUP_READY: &str = "Ready. Run `spendbranch status` anytime.";
pub const SETUP_PROBLEMS: &str =
    "Finished with problems: see the marks above, then run `spendbranch status`.";

pub fn scheduler_failed(why: &str) -> String {
    format!("Scheduled sync: not installed ({why}). Hooks still send usage when a session ends.")
}

pub fn scheduler_not_removed(why: &str) -> String {
    format!("Could not remove the scheduled sync: {why}")
}

// --- uninstall ------------------------------------------------------------------------

pub const UNINSTALL_WHAT: &str = "uninstall";
pub const UNINSTALL_ASK: &str = "This removes SpendBranch from this computer: it disconnects the device and\nremoves the agent hooks, the scheduled sync, the PATH lines, the data folder\nand the program.\n\nContinue? [y/N] ";
pub const UNINSTALL_CANCELLED: &str = "Cancelled. Nothing was removed.";
pub const UNINSTALL_NEEDS_YES: &str =
    "Not a terminal: run `spendbranch uninstall --yes` to remove SpendBranch without a question.";
pub const UNINSTALL_DONE: &str = "SpendBranch was removed from this computer.";
pub const UNINSTALL_PARTIAL: &str =
    "SpendBranch was removed, except what the marked lines above say is left.";

pub const UNINSTALL_REVOKING: &str = "revoking this device on the server";
pub const UNINSTALL_REVOKED: &str = "device revoked on the server";
pub const UNINSTALL_ALREADY_REVOKED: &str = "the device was already revoked or expired";
pub const UNINSTALL_NOT_REVOKED: &str =
    "the server could not revoke it: revoke the device in \"My devices\" on the web";
pub const UNINSTALL_NOT_CONNECTED: &str = "this device was not connected";
pub const UNINSTALL_NO_HOOKS: &str = "none were installed";
pub const UNINSTALL_SCHEDULE_REMOVED: &str = "the sync every 30 minutes and at login is gone";
pub const UNINSTALL_NO_SCHEDULE: &str = "none was installed";
pub const UNINSTALL_NOTHING: &str = "nothing to remove";
pub const UNINSTALL_HOMEBREW: &str = "installed by Homebrew: run `brew uninstall spendbranch`";

pub fn uninstall_key_kept(why: &str) -> String {
    format!("the stored key could not be deleted: {why}")
}

pub fn uninstall_hooks_removed(agents: &str) -> String {
    format!("removed from {agents}")
}

pub fn uninstall_path_files(files: &str) -> String {
    format!("removed the line from {files}")
}

pub fn uninstall_path_entry(dir: &str) -> String {
    format!("removed {dir} from your user Path")
}

pub fn uninstall_deleted(path: &str) -> String {
    format!("deleted {path}")
}

pub fn uninstall_data_not_ours(dir: &str) -> String {
    format!("left {dir} as it is: SPENDBRANCH_HOME is not a folder named spendbranch")
}

pub fn uninstall_failed(what: &str, why: &str) -> String {
    format!("{what}: {why}")
}

// --- otel and relay -------------------------------------------------------------------

pub const OTEL_WARNING: &str = "Do not turn on OTEL_LOG_USER_PROMPTS, OTEL_LOG_TOOL_DETAILS or OTEL_LOG_RAW_API_BODIES. OpenTelemetry carries no history: `spendbranch sync` stays the source of the final numbers.";

pub const EXE_PATH_UNSAFE: &str = "The path of this program or SPENDBRANCH_HOME has a line break or a character the system would expand (on Windows: \" % $ `). Move it to a plain path and run this again.";

pub const NO_SERVER: &str =
    "No server to pair with: pass `--server URL` or set SPENDBRANCH_SERVER.";

pub const MEMBER_UNKNOWN: &str = "Member: unknown (the server did not say who). Whoever approved the code may not be you: answer yes only if you just approved it yourself.";

pub const RELAY_NEEDS_OTEL: &str = "Run `spendbranch otel claude-code` first: it picks the port and the token the relay checks, and writes them where Claude Code reads them.";

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
        "Wrote {file}.\nLoad it in the terminal where you start Claude Code:\n  {load}\nand keep `spendbranch relay` running while you work. The file holds the relay's token, readable only by you: the relay signs nothing that comes without it. Running `otel` again picks a new port and token: restart a relay that is already running, and reload the file where you start Claude Code."
    )
}

pub fn relay_listening(port: u16, agent_port: u16, server: &str) -> String {
    let mut text = format!(
        "Relay listening on http://127.0.0.1:{port}{} and forwarding to {server}. Press Ctrl+C to stop.",
        crate::relay::PATH
    );
    if port != agent_port {
        text.push_str(&format!(
            "\n`spendbranch otel` gave the agent port {agent_port}: point its OTEL_EXPORTER_OTLP_LOGS_ENDPOINT at this port."
        ));
    }
    text
}

/// `agents` when the busy port is the one the agent sends to: whoever holds it gets the export.
pub fn relay_port_busy(port: u16, agents: bool, why: &str) -> String {
    if agents {
        format!(
            "Cannot listen on 127.0.0.1:{port}: {why}. Another program holds the port Claude Code sends to, and it receives what Claude Code exports. Stop it, or run `spendbranch otel claude-code` again for a new port and token and reload the file where you start Claude Code."
        )
    } else {
        format!("Cannot listen on 127.0.0.1:{port}: {why}. Pick another port with `--port`.")
    }
}

/// Dropped batches are said on the first and then on every 50th, so a long outage stays readable.
// `is_multiple_of` is Rust 1.90; the MSRV is 1.89.
#[allow(clippy::manual_is_multiple_of)]
pub fn report_drop(n: u64) -> bool {
    n == 1 || n % 50 == 0
}

pub fn relay_stopped(why: &str) -> String {
    format!("The relay stopped: {why}")
}

pub fn relay_dropped(n: u64, why: &str) -> String {
    format!("Dropped {n} batch(es) so far: {why}")
}

/// Values of a known field that came in another shape (a negative counter, an id out of its
/// pattern): said with the same pace as dropped batches, as a count only.
pub fn relay_malformed(n: u64) -> String {
    format!("Left out {n} malformed value(s) so far: the agent's export may be broken.")
}
