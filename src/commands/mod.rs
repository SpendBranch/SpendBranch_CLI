//! One function per command; they orchestrate, the business rules live elsewhere.
//! F1 covers everything that needs no network: usage checks and the "not connected"
//! answers. Commands that need the server end in `NOT_AVAILABLE` until their phase
//! (login/status/logout F5, sync F6, hooks and setup F7, otel/relay F8).
use crate::cli::{Env, args::Command, messages as m};
use crate::infra::{lock, paths, store::Store};
use crate::sources::Source;
use std::{error::Error, io::Write};

pub struct Ctx<'a> {
    pub env: Env<'a>,
    pub out: &'a mut dyn Write,
}

type Result<T> = std::result::Result<T, Box<dyn Error>>;

pub fn run(command: Command, ctx: &mut Ctx) -> Result<i32> {
    match command {
        Command::Setup { source, .. } => match source.as_deref() {
            Some(s) if !matches!(s, "claude-code" | "codex") => say(ctx, m::USAGE_SETUP, 2),
            _ => say(ctx, m::NOT_AVAILABLE, 1),
        },
        Command::Login { .. } => say(ctx, m::NOT_AVAILABLE, 1),
        Command::Status | Command::Relay { .. } => require_connection(ctx),
        Command::Sync => sync(ctx),
        Command::Enable(s) => source_command(ctx, s, m::USAGE_ENABLE),
        Command::Disable(s) => source_command(ctx, s, m::USAGE_DISABLE),
        Command::Hooks { action, source } => {
            let valid = matches!(action.as_deref(), Some("install" | "uninstall"))
                && source.as_deref().and_then(Source::parse).is_some();
            say(
                ctx,
                if valid {
                    m::NOT_AVAILABLE
                } else {
                    m::USAGE_HOOKS
                },
                if valid { 1 } else { 2 },
            )
        }
        // Always exits 0 and never writes: it must not disturb the agent. Real work in F7.
        Command::Hook { agent, .. } => Ok(
            if matches!(agent.as_deref(), Some("claude-code" | "codex")) {
                0
            } else {
                2
            },
        ),
        Command::Otel(s) => otel(ctx, s),
        Command::Logout => {
            if connected(ctx)? {
                say(ctx, m::NOT_AVAILABLE, 1)
            } else {
                say(ctx, m::ALREADY_DISCONNECTED, 0)
            }
        }
        Command::Help | Command::Version | Command::Unknown(_) => {
            unreachable!("handled by cli::run")
        }
    }
}

fn say(ctx: &mut Ctx, text: &str, code: i32) -> Result<i32> {
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

fn source_command(ctx: &mut Ctx, source: Option<String>, usage: &str) -> Result<i32> {
    if source.as_deref().and_then(Source::parse).is_none() {
        return say(ctx, usage, 2);
    }
    require_connection(ctx)
}

fn otel(ctx: &mut Ctx, source: Option<String>) -> Result<i32> {
    match source.map(|s| s.replace('-', "_")).as_deref() {
        Some("claude_code") => require_connection(ctx),
        Some("codex") => say(ctx, m::OTEL_CODEX_REFUSED, 2),
        _ => say(ctx, m::USAGE_OTEL, 2),
    }
}

fn sync(ctx: &mut Ctx) -> Result<i32> {
    let dir = paths::data_dir(ctx.env)?;
    let Some(_lock) = lock::try_acquire(&dir)? else {
        return say(ctx, m::SYNC_RUNNING, 0);
    };
    require_connection(ctx)
}
