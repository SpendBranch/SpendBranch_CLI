//! One function per command; they orchestrate, the business rules live elsewhere. Files,
//! sockets, processes and SQLite are reached only through `infra`.
mod account;
mod home;
mod hook;
mod hooks;
mod otel;
mod relay;
mod setup;
mod sync;
mod uninstall;

use crate::cli::{args::Command, messages as m};
use crate::env::Env;
use crate::infra::system::System;
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
    /// How screens are drawn: color, borders and marks.
    pub style: crate::cli::screen::Style,
}

pub use home::home;

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
        Command::Relay { port } => relay::run(ctx, port),
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
        Command::Uninstall { yes } => uninstall::run(ctx, yes),
    }
}

/// Runs `work` on its own thread while a spinner turns on the terminal (`label`, `detail`),
/// then wipes the spinner's line. Off a terminal it just runs `work`.
pub(super) fn spinning<T: Send>(
    ctx: &mut Ctx,
    label: &str,
    detail: &str,
    work: &(dyn Fn() -> T + Sync),
) -> Result<T> {
    if !ctx.style.tty {
        return Ok(work());
    }
    std::thread::scope(|scope| {
        let running = scope.spawn(work);
        let mut tick = 0;
        let mut shown = 0;
        while !running.is_finished() {
            let line = crate::cli::screen::spinner(ctx.style, tick, label, detail);
            shown = crate::cli::screen::width(&line);
            write!(ctx.out, "\r{line}")?;
            ctx.out.flush()?;
            (ctx.sleep)(Duration::from_millis(100));
            tick += 1;
        }
        if shown > 0 {
            write!(ctx.out, "\r{}\r", " ".repeat(shown))?;
        }
        running
            .join()
            .map_err(|_| Box::<dyn Error>::from("the work behind the spinner stopped"))
    })
}

pub(super) fn say(ctx: &mut Ctx, text: &str, code: i32) -> Result<i32> {
    writeln!(ctx.out, "{text}")?;
    Ok(code)
}

fn otel(ctx: &mut Ctx, source: Option<String>) -> Result<i32> {
    match source.map(|s| s.replace('-', "_")).as_deref() {
        Some("claude_code") => otel::run(ctx),
        Some("codex") => say(ctx, m::OTEL_CODEX_REFUSED, 2),
        _ => say(ctx, m::USAGE_OTEL, 2),
    }
}
