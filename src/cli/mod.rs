//! The command line: argv in, exit code out.
pub mod args;
pub mod messages;

use crate::commands::{self, Ctx};
use crate::env::Env;
use args::Parsed;
use std::io::Write;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Exit codes: 0 success, 1 failure, 2 wrong usage.
pub fn run(args: &[String], env: Env, out: &mut dyn Write) -> i32 {
    match dispatch(args, env, out) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("unexpected error: {e}");
            1
        }
    }
}

fn dispatch(
    args: &[String],
    env: Env,
    out: &mut dyn Write,
) -> Result<i32, Box<dyn std::error::Error>> {
    let help = messages::help(VERSION);
    let command = match args::parse(args) {
        Ok(c) => c,
        Err(usage) => {
            writeln!(out, "{usage}\n\n{help}")?;
            return Ok(2);
        }
    };
    match command {
        Parsed::Version => writeln!(out, "{VERSION}")?,
        Parsed::Help => writeln!(out, "{help}")?,
        Parsed::Unknown(name) => {
            writeln!(out, "Unknown command: {name}.\n\n{help}")?;
            return Ok(2);
        }
        Parsed::Run(command) => return commands::run(command, &mut Ctx { env, out }),
    }
    Ok(0)
}
