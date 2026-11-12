//! One function per command; they orchestrate, the business rules live elsewhere.
//! F1 covers everything that needs no network: usage checks and the "not connected"
//! answers. Commands that need the server end in `NOT_AVAILABLE` until their phase
//! (otel/relay F8).
mod account;
mod hook;
mod hooks;
mod setup;
mod sync;

use crate::cli::{args::Command, messages as m};
use crate::env::Env;
use crate::infra::{paths, store::Store, system::System};
use crate::sources::Source;
use std::{
    error::Error,
    io::{BufRead, Write},
    time::Duration,
};

/// Everything a command touches besides the data folder: the screen, the keyboard, the clock's
/// sleep and the browser. Tests give their own, so nothing waits and nothing opens.
pub struct Ctx<'a> {
    pub env: Env<'a>,
    pub out: &'a mut dyn Write,
    pub input: &'a mut dyn BufRead,
    pub sleep: &'a dyn Fn(Duration),
    pub open_url: &'a dyn Fn(&str),
    /// Standard output is a terminal: the browser may be opened.
    pub interactive: bool,
    /// Processes and system tools.
    pub system: &'a dyn System,
}

pub(super) type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn run(command: Command, ctx: &mut Ctx) -> Result<i32> {
    match command {
        Command::Setup {
            server,
            source,
            relogin,
        } => match source.as_deref().map(Source::parse) {
            Some(None) => say(ctx, m::USAGE_SETUP, 2),
            parsed => setup::run(ctx, server, parsed.flatten(), relogin),
        },
        Command::Login { server } => account::login(ctx, server),
        Command::Status => account::status(ctx),
        Command::Relay { .. } => require_connection(ctx),
        Command::Sync => sync::run(ctx),
        Command::Enable(s) => match s.as_deref().and_then(Source::parse) {
            Some(source) => account::enable(ctx, source),
            None => say(ctx, m::USAGE_ENABLE, 2),
        },
        Command::Disable(s) => match s.as_deref().and_then(Source::parse) {
            Some(source) => account::disable(ctx, source),
            None => say(ctx, m::USAGE_DISABLE, 2),
        },
        Command::Hooks { action, source } => {
            match (action.as_deref(), source.as_deref().and_then(Source::parse)) {
                (Some("install"), Some(s)) => hooks::install(ctx, s),
                (Some("uninstall"), Some(s)) => hooks::uninstall(ctx, s),
                _ => say(ctx, m::USAGE_HOOKS, 2),
            }
        }
        Command::Hook { agent, event } => hook::run(ctx, agent.as_deref(), event.as_deref()),
        Command::Otel(s) => otel(ctx, s),
        Command::Logout => account::logout(ctx),
    }
}

pub(super) fn say(ctx: &mut Ctx, text: &str, code: i32) -> Result<i32> {
    writeln!(ctx.out, "{text}")?;
    Ok(code)
}

fn connected(ctx: &Ctx) -> Result<bool> {
    Ok(Store::open(&paths::data_dir(ctx.env)?)?.is_connected()?)
}

/// Commands that only make sense once paired.
fn require_connection(ctx: &mut Ctx) -> Result<i32> {
    if connected(ctx)? {
        say(ctx, m::NOT_AVAILABLE, 1)
    } else {
        say(ctx, m::NOT_CONNECTED, 1)
    }
}

fn otel(ctx: &mut Ctx, source: Option<String>) -> Result<i32> {
    match source.map(|s| s.replace('-', "_")).as_deref() {
        Some("claude_code") => require_connection(ctx),
        Some("codex") => say(ctx, m::OTEL_CODEX_REFUSED, 2),
        _ => say(ctx, m::USAGE_OTEL, 2),
    }
}
