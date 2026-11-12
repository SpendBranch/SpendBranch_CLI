//! The command line: argv in, exit code out.
pub mod args;
pub mod messages;
pub mod render;

use crate::commands::{self, Ctx};
use crate::env::Env;
use args::Parsed;
use std::{
    io::{BufRead, Write},
    time::Duration,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a run needs from the outside world besides the environment. The real ones are made
/// in `main`; `run` gives inert ones, so nothing waits, reads the keyboard or opens a browser.
pub struct Io<'a> {
    pub out: &'a mut dyn Write,
    pub input: &'a mut dyn BufRead,
    pub sleep: &'a dyn Fn(Duration),
    pub open_url: &'a dyn Fn(&str),
    pub interactive: bool,
}

/// Exit codes: 0 success, 1 failure, 2 wrong usage.
pub fn run(args: &[String], env: Env, out: &mut dyn Write) -> i32 {
    let mut io = Io {
        out,
        input: &mut std::io::empty(),
        sleep: &|_| {},
        open_url: &|_| {},
        interactive: false,
    };
    run_with(args, env, &mut io)
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

fn dispatch(args: &[String], env: Env, io: &mut Io) -> Result<i32, Box<dyn std::error::Error>> {
    let help = messages::help(VERSION);
    let command = match args::parse(args) {
        Ok(c) => c,
        Err(usage) => {
            writeln!(io.out, "{usage}\n\n{help}")?;
            return Ok(2);
        }
    };
    match command {
        Parsed::Version => writeln!(io.out, "{VERSION}")?,
        Parsed::Help => writeln!(io.out, "{help}")?,
        Parsed::Unknown(name) => {
            writeln!(io.out, "Unknown command: {name}.\n\n{help}")?;
            return Ok(2);
        }
        Parsed::Run(command) => {
            let mut ctx = Ctx {
                env,
                out: &mut *io.out,
                input: &mut *io.input,
                sleep: io.sleep,
                open_url: io.open_url,
                interactive: io.interactive,
            };
            return commands::run(command, &mut ctx);
        }
    }
    Ok(0)
}
