//! Turns argv into a `Command`. Pure: no I/O, no process exit.
use lexopt::prelude::*;

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    /// No arguments at all: the start screen on a terminal, the help otherwise.
    Home,
    Help,
    /// `<command> --help`.
    CommandHelp(&'static str),
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

/// `Err` carries what was wrong; the caller prints it as a usage error and exits 2.
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
    if help {
        return Ok(command
            .and_then(|c| COMMANDS.into_iter().find(|k| *k == c))
            .map_or(Parsed::Help, Parsed::CommandHelp));
    }
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
        Some(other) => return Ok(Parsed::Unknown(other.to_string())),
        None => return Ok(Parsed::Home),
    };
    Ok(Parsed::Run(run))
}

/// The commands a person types (`hook` is the agents' own, so it is left out).
pub const COMMANDS: [&str; 10] = [
    "setup", "login", "status", "sync", "enable", "disable", "hooks", "otel", "relay", "logout",
];

/// The command a typo most likely meant: at most 2 edits away.
pub fn suggest(word: &str) -> Option<&'static str> {
    COMMANDS
        .into_iter()
        .map(|c| (distance(word, c), c))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Levenshtein distance, one row at a time.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(above + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typo_suggests_the_nearest_command_and_nonsense_suggests_nothing() {
        assert_eq!(suggest("stauts"), Some("status"));
        assert_eq!(suggest("setpu"), Some("setup"));
        assert_eq!(suggest("logot"), Some("logout"));
        assert_eq!(suggest("frobnicate"), None);
        assert_eq!(distance("", "sync"), 4);
    }
}
