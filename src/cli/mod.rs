//! The command line: argv in, exit code out.
pub mod args;
pub mod mascot;
pub mod messages;
pub mod render;
pub mod screen;

use crate::commands::{self, Ctx};
use crate::env::Env;
use crate::infra::system::System;
use args::Parsed;
use screen::Style;
use std::{
    io::{BufRead, Write},
    time::Duration,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a run needs from the outside world besides the environment. The real ones are made
/// in `main`; tests pass their own.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub input: &'a mut dyn BufRead,
    pub sleep: &'a dyn Fn(Duration),
    pub open_url: &'a dyn Fn(&str),
    pub interactive: bool,
    pub system: &'a dyn System,
}

/// Exit codes: 0 success, 1 failure, 2 wrong usage.
/// The one place that decides color: only where the terminal says it shows truecolor
/// (`COLORTERM`, or Windows Terminal's `WT_SESSION`), never
/// with `NO_COLOR` or `TERM=dumb`.
pub fn truecolor(env: Env) -> bool {
    env("NO_COLOR").is_none_or(|v| v.is_empty())
        && env("TERM").as_deref() != Some("dumb")
        && (matches!(env("COLORTERM").as_deref(), Some("truecolor" | "24bit"))
            // Windows Terminal shows truecolor but sets no COLORTERM.
            || env("WT_SESSION").is_some_and(|v| !v.is_empty()))
}

pub fn run_with(args: &[String], env: Env, io: &mut Io) -> i32 {
    match dispatch(args, env, io) {
        Ok(code) => code,
        Err(e) => {
            // `eprintln!` panics when stderr cannot be written; a failed message is no reason to crash.
            let _ = writeln!(std::io::stderr(), "unexpected error: {e}");
            1
        }
    }
}

fn ctx<'b>(env: Env<'b>, io: &'b mut Io) -> Ctx<'b> {
    Ctx {
        env,
        out: &mut *io.out,
        input: &mut *io.input,
        sleep: io.sleep,
        open_url: io.open_url,
        interactive: io.interactive,
        system: io.system,
    }
}

fn dispatch(args: &[String], env: Env, io: &mut Io) -> Result<i32, Box<dyn std::error::Error>> {
    // Borders and the owl only on a terminal; color only where truecolor is known to work.
    let st = Style {
        color: io.interactive && truecolor(env),
        tty: io.interactive,
    };
    let command = match args::parse(args) {
        Ok(c) => c,
        Err(why) => {
            writeln!(io.out, "{}", screen::usage_error(st, &why, None))?;
            return Ok(2);
        }
    };
    let text = match command {
        Parsed::Version => VERSION.to_owned(),
        Parsed::Home if io.interactive => screen::home(st, &commands::home(&ctx(env, io))),
        Parsed::Home | Parsed::Help => screen::help(st),
        Parsed::CommandHelp(name) => match messages::command_help(name) {
            Some(h) => screen::command_help(st, h),
            None => screen::help(st),
        },
        Parsed::Unknown(name) => {
            let what = messages::unknown_command(&name);
            writeln!(
                io.out,
                "{}",
                screen::usage_error(st, &what, args::suggest(&name))
            )?;
            return Ok(2);
        }
        Parsed::Run(command) => return commands::run(command, &mut ctx(env, io)),
    };
    writeln!(io.out, "{text}")?;
    Ok(0)
}
