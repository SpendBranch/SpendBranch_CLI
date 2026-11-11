//! Turns argv into a `Command`. Pure: no I/O, no process exit.
use lexopt::prelude::*;

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Help,
    Version,
    /// A word that is not a command.
    Unknown(String),
    Run(Command),
}

/// A command that does work. Help, version and unknown words are not commands.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Setup {
        server: Option<String>,
        source: Option<String>,
        relogin: bool,
    },
    Login {
        server: Option<String>,
    },
    Status,
    Sync,
    Enable(Option<String>),
    Disable(Option<String>),
    Hooks {
        action: Option<String>,
        source: Option<String>,
    },
    Hook {
        agent: Option<String>,
        event: Option<String>,
    },
    Otel(Option<String>),
    Relay {
        port: Option<u16>,
    },
    Logout,
}

/// `Err` carries a usage message; the caller prints it with the help and exits 2.
pub fn parse(args: &[String]) -> Result<Parsed, String> {
    let (mut server, mut source, mut port) = (None, None, None);
    let (mut relogin, mut version, mut help) = (false, false, false);
    let mut positionals = Vec::new();
    let mut p = lexopt::Parser::from_args(args);
    let e = |e: lexopt::Error| e.to_string();
    while let Some(arg) = p.next().map_err(e)? {
        match arg {
            Long("server") => server = Some(p.value().map_err(e)?.string().map_err(e)?),
            Long("source") => source = Some(p.value().map_err(e)?.string().map_err(e)?),
            Long("port") => port = Some(p.value().map_err(e)?.string().map_err(e)?),
            Long("relogin") => relogin = true,
            Short('v') | Long("version") => version = true,
            Short('h') | Long("help") => help = true,
            Value(v) => positionals.push(v.string().map_err(e)?),
            other => return Err(other.unexpected().to_string()),
        }
    }
    if version {
        return Ok(Parsed::Version);
    }
    let mut pos = positionals.into_iter();
    let (command, a1, a2) = (pos.next(), pos.next(), pos.next());
    let run = match command.as_deref() {
        Some("login") => Command::Login { server },
        Some("setup") => Command::Setup {
            server,
            source,
            relogin,
        },
        Some("status") => Command::Status,
        Some("sync") => Command::Sync,
        Some("enable") => Command::Enable(a1),
        Some("disable") => Command::Disable(a1),
        Some("hooks") => Command::Hooks {
            action: a1,
            source: a2,
        },
        Some("hook") => Command::Hook {
            agent: a1,
            event: a2,
        },
        Some("otel") => Command::Otel(a1),
        Some("relay") => Command::Relay {
            port: port
                .map(|p| p.parse::<u16>().map_err(|_| format!("invalid port '{p}'")))
                .transpose()?,
        },
        Some("logout") => Command::Logout,
        Some(other) if !help => return Ok(Parsed::Unknown(other.to_string())),
        _ => return Ok(Parsed::Help),
    };
    Ok(Parsed::Run(run))
}
