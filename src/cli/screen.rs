//! The screens: boxes with joined borders, rules, marks and colors. Pure: no I/O.
//! At most 80 columns. Color is decoration, never information: off a terminal every screen
//! is plain text with `[ok] [!] [x]` marks, no borders and no owl.
use super::{VERSION, mascot, messages as m};

pub const WIDTH: usize = 80;
/// A line that becomes a rule joined to the box's sides.
pub const DIVIDER: &str = "---";

#[derive(Clone, Copy)]
pub struct Style {
    /// Escape codes (truecolor terminal, no `NO_COLOR`).
    pub color: bool,
    /// A terminal: borders, rules and the owl.
    pub tty: bool,
}

impl Style {
    fn paint(self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_owned()
        }
    }
    /// The owl's amber: commands and titles.
    pub fn accent(self, s: &str) -> String {
        self.paint("1;38;2;240;176;32", s)
    }
    /// The owl's brown: borders and rules.
    fn line(self, s: &str) -> String {
        self.paint("38;2;156;112;68", s)
    }
    pub fn dim(self, s: &str) -> String {
        self.paint("2", s)
    }
    pub fn bold(self, s: &str) -> String {
        self.paint("1", s)
    }
    /// A command to type: amber in color, in backticks without.
    pub fn cmd(self, s: &str) -> String {
        if self.color {
            self.accent(s)
        } else {
            format!("`{s}`")
        }
    }
    pub fn mark(self, code: &str, glyph: &str, plain: &str) -> String {
        if self.color {
            self.paint(code, glyph)
        } else {
            plain.to_owned()
        }
    }
    pub fn ok(self) -> String {
        self.mark("32", "✓", "[ok]")
    }
    pub fn warn(self) -> String {
        self.mark("33", "!", "[!]")
    }
    pub fn bad(self) -> String {
        self.mark("31", "✗", "[x]")
    }
}

/// Columns a string takes: escape codes take none.
pub fn width(s: &str) -> usize {
    let mut n = 0;
    let mut escape = false;
    for c in s.chars() {
        if escape {
            escape = c != 'm';
        } else if c == '\x1b' {
            escape = true;
        } else {
            n += 1;
        }
    }
    n
}

fn pad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(width(s))))
}

/// Plain text cut to `w` columns.
pub fn fit(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_owned()
    } else {
        s.chars().take(w - 1).chain(['…']).collect()
    }
}

/// Plain text broken into lines of at most `w` columns, at spaces; a longer word is cut.
pub fn wrap(text: &str, w: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let mut word: Vec<char> = word.chars().collect();
        while word.len() > w {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            lines.push(word.drain(..w).collect());
        }
        let word: String = word.into_iter().collect();
        if line.is_empty() {
            line = word;
        } else if line.chars().count() + 1 + word.chars().count() <= w {
            line.push(' ');
            line.push_str(&word);
        } else {
            lines.push(std::mem::replace(&mut line, word));
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// `╭─── title ───╮`, with a `┬` after `split` inner columns when the box has two panes.
fn top(st: Style, title: &str, split: Option<usize>) -> String {
    let inner = WIDTH - 2;
    let head = 5 + width(title);
    let rest = match split {
        Some(left) => format!(
            "{}┬{}",
            "─".repeat(left - head),
            "─".repeat(inner - left - 1)
        ),
        None => "─".repeat(inner - head),
    };
    format!(
        "{} {} {}",
        st.line("╭───"),
        st.accent(title),
        st.line(&format!("{rest}╮"))
    )
}

fn side(st: Style) -> String {
    st.line("│")
}

/// A full-width box. Off a terminal: the lines alone, a divider as a blank line.
pub fn boxed(st: Style, title: &str, lines: &[String]) -> String {
    if !st.tty {
        let plain: Vec<&str> = lines
            .iter()
            .map(|l| {
                if l == DIVIDER {
                    ""
                } else {
                    l.strip_prefix(' ').unwrap_or(l)
                }
            })
            .collect();
        return plain.join("\n").trim_matches('\n').to_owned();
    }
    let inner = WIDTH - 2;
    let mut out = vec![top(st, title, None)];
    for l in lines {
        out.push(if l == DIVIDER {
            st.line(&format!("├{}┤", "─".repeat(inner)))
        } else {
            format!("{}{}{}", side(st), pad(l, inner), side(st))
        });
    }
    out.push(st.line(&format!("╰{}╯", "─".repeat(inner))));
    out.join("\n")
}

/// The owl on the left, `right` beside it; a divider splits only the right pane.
fn panel(st: Style, title: &str, right: &[String]) -> String {
    const LEFT: usize = 40;
    let rw = WIDTH - 3 - LEFT;
    let owl: Vec<String> = mascot::owl_lines(st.color)
        .into_iter()
        .map(|l| format!("  {l}"))
        .collect();
    let mut out = vec![top(st, title, Some(LEFT))];
    for i in 0..owl.len().max(right.len()) {
        let left = pad(owl.get(i).map_or("", String::as_str), LEFT);
        out.push(match right.get(i).map(String::as_str) {
            Some(DIVIDER) => format!(
                "{}{left}{}",
                side(st),
                st.line(&format!("├{}┤", "─".repeat(rw)))
            ),
            r => {
                let r = r
                    .filter(|r| !r.is_empty())
                    .map_or(String::new(), |r| format!("  {r}"));
                format!("{}{left}{}{}{}", side(st), side(st), pad(&r, rw), side(st))
            }
        });
    }
    out.push(st.line(&format!("╰{}┴{}╯", "─".repeat(LEFT), "─".repeat(rw))));
    out.join("\n")
}

// --- the start screen -----------------------------------------------------------------

/// One agent on the start screen.
pub struct HomeAgent {
    pub name: &'static str,
    pub arg: &'static str,
    pub reading: bool,
    pub hooked: bool,
}

/// What the start screen knows, from the local state only.
pub enum Home {
    NotConnected,
    Connected {
        organization: String,
        agents: Vec<HomeAgent>,
        waiting: u64,
        /// "3 h ago".
        last_sent: Option<String>,
    },
    /// The data folder is there but could not be read.
    Unreadable,
}

/// The pane width minus its 2-column margin.
const RIGHT_TEXT: usize = WIDTH - 3 - 40 - 2;

fn home_right(st: Style, home: &Home) -> Vec<String> {
    let mut r = vec![String::new()];
    match home {
        Home::NotConnected => {
            r.push(format!("{} {}", st.bad(), m::HOME_NOT_CONNECTED));
            r.push(String::new());
            r.push(st.bold(m::HOME_START));
            r.push(st.accent("spendbranch setup"));
            r.push(st.dim(m::HOME_SETUP_WHAT));
            r.push(DIVIDER.into());
            let [title, rest @ ..] = m::HOME_PRIVACY;
            r.push(st.bold(title));
            r.extend(rest.iter().map(|l| st.dim(l)));
        }
        Home::Unreadable => {
            r.push(format!("{} {}", st.warn(), m::HOME_UNREADABLE));
            r.push(String::new());
            r.push(st.accent("spendbranch status"));
        }
        Home::Connected {
            organization,
            agents,
            waiting,
            last_sent,
        } => {
            let mark = width(&st.ok()) + 1;
            r.push(format!(
                "{} {}",
                st.ok(),
                fit(organization, RIGHT_TEXT - mark)
            ));
            r.push(st.dim(m::HOME_CONNECTED));
            r.push(DIVIDER.into());
            r.push(st.bold("Agents"));
            for a in agents {
                let (dot, state) = match (a.reading, a.hooked) {
                    (true, true) => (st.mark("32", "●", "[ok]"), "reading · hook"),
                    (true, false) => (st.mark("33", "●", "[!]"), "reading · no hook"),
                    (false, _) => (st.mark("2", "○", "[-]"), "off"),
                };
                r.push(format!("{dot} {}{}", pad(a.name, 13), st.dim(state)));
            }
            r.push(DIVIDER.into());
            r.push(st.bold("Upload"));
            let sent = last_sent
                .as_deref()
                .map_or(String::new(), |s| st.dim(&format!(" · sent {s}")));
            r.push(format!("{waiting} waiting{sent}"));
            r.push(DIVIDER.into());
            r.push(st.bold("Next"));
            let next = match agents.iter().find(|a| a.reading && !a.hooked) {
                Some(a) => format!("spendbranch hooks install {}", a.arg),
                None if agents.iter().all(|a| !a.reading) => "spendbranch setup".into(),
                None => "spendbranch status".into(),
            };
            r.push(st.accent(&next));
        }
    }
    r
}

/// `spendbranch` alone, on a terminal.
pub fn home(st: Style, home: &Home) -> String {
    format!(
        "{}\n  {}{}{}",
        panel(
            st,
            &format!("SpendBranch CLI v{VERSION}"),
            &home_right(st, home)
        ),
        st.dim("Run "),
        st.cmd("spendbranch --help"),
        st.dim(" for all commands")
    )
}

// --- help -----------------------------------------------------------------------------

/// `spendbranch --help`: the commands in groups.
pub fn help(st: Style) -> String {
    let mut out = vec![
        format!(
            "{} {VERSION} {}",
            st.accent("spendbranch"),
            st.dim(m::TAGLINE)
        ),
        String::new(),
    ];
    for (group, note, commands) in m::HELP_GROUPS {
        let head = if note.is_empty() {
            st.bold(group)
        } else {
            format!("{} {}", st.bold(group), st.dim(note))
        };
        out.push(if st.tty {
            format!("{head} {}", st.line(&"─".repeat(WIDTH - width(&head) - 1)))
        } else {
            head
        });
        for (command, what) in commands {
            out.push(format!(
                "  {}{}",
                pad(&st.accent(command), 25),
                st.dim(what)
            ));
        }
        out.push(String::new());
    }
    out.extend(m::PRIVACY.iter().map(|l| st.dim(l)));
    out.push(if st.tty {
        st.line(&"─".repeat(WIDTH))
    } else {
        String::new()
    });
    out.push(format!(
        "Run {} for details.",
        st.cmd("spendbranch <command> --help")
    ));
    out.join("\n")
}

/// `spendbranch <name> --help`.
pub fn command_help(st: Style, h: &m::CommandHelp) -> String {
    let name = format!("spendbranch {}", h.name);
    let usage = if h.usage.is_empty() {
        st.accent(&name)
    } else {
        format!("{} {}", st.accent(&name), h.usage)
    };
    let mut lines = vec![String::new(), format!(" {usage}"), String::new()];
    lines.extend(h.about.iter().map(|l| format!(" {l}")));
    lines.push(String::new());
    if !h.options.is_empty() {
        lines.push(DIVIDER.into());
        lines.push(String::new());
        for (flag, what) in h.options {
            lines.push(format!(" {}{}", pad(&st.accent(flag), 18), st.dim(what)));
        }
        lines.push(String::new());
    }
    boxed(st, &name, &lines)
}

// --- steps (setup) --------------------------------------------------------------------

/// A step starting, before what it prints as it goes.
pub fn step_start(st: Style, label: &str) -> String {
    format!(
        "{} {}",
        st.mark("1;38;2;240;176;32", "●", "*"),
        st.bold(label)
    )
}

/// How a step ended: its mark, its name and one line of detail.
pub fn step(st: Style, mark: &str, label: &str, detail: &str) -> String {
    let lead = format!("{mark} {}", st.bold(label));
    format!("{} {detail}", pad(&lead, width(mark) + 1 + 11))
        .trim_end()
        .to_owned()
}

/// A step still running: a turning mark in amber, its name and what it waits for. Braille dots
/// where the terminal shows color and symbols, `| / - \` everywhere else.
pub fn spinner(st: Style, tick: usize, label: &str, detail: &str) -> String {
    const DOTS: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    const BARS: [&str; 4] = ["|", "/", "-", "\\"];
    let frame = if st.color {
        DOTS[tick % DOTS.len()]
    } else {
        BARS[tick % BARS.len()]
    };
    step(
        st,
        &st.mark("1;38;2;240;176;32", frame, frame),
        label,
        &st.dim(detail),
    )
}

/// A title above a command's steps: the program, its version and what it does.
pub fn title(st: Style, what: &str) -> String {
    format!(
        "{} {}",
        st.accent(&format!("SpendBranch CLI {VERSION}")),
        st.dim(&format!("· {what}"))
    )
}

/// What goes before each line a step printed: the step line goes on down past it.
pub fn nest(st: Style) -> String {
    if st.tty {
        format!("{}   ", st.line("│"))
    } else {
        "    ".into()
    }
}

/// The line between two steps; nothing off a terminal.
pub fn joint(st: Style) -> Option<String> {
    st.tty.then(|| st.line("│"))
}

/// A usage error: what was wrong, a guess when there is one, and where the help is.
pub fn usage_error(st: Style, what: &str, guess: Option<&str>) -> String {
    let mut out = vec![format!("{} {what}", st.mark("31", "error:", "error:"))];
    if let Some(g) = guess {
        let joint = if st.tty {
            st.line("╰─") + " d"
        } else {
            "D".into()
        };
        out.push(format!(
            "  {joint}id you mean {}?",
            st.cmd(&format!("spendbranch {g}"))
        ));
    }
    out.push(String::new());
    out.push(format!(
        "Run {} to see all commands.",
        st.cmd("spendbranch --help")
    ));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: [Style; 3] = [
        Style {
            color: true,
            tty: true,
        },
        Style {
            color: false,
            tty: true,
        },
        Style {
            color: false,
            tty: false,
        },
    ];

    fn agent(name: &'static str, reading: bool, hooked: bool) -> HomeAgent {
        HomeAgent {
            name,
            arg: "codex",
            reading,
            hooked,
        }
    }

    fn every_screen(st: Style) -> Vec<String> {
        let connected = Home::Connected {
            organization: "A very long organization name that does not fit".into(),
            agents: vec![
                agent("Claude Code", true, true),
                agent("Codex", true, false),
            ],
            waiting: 12,
            last_sent: Some("3 h ago".into()),
        };
        let mut out = vec![
            help(st),
            usage_error(st, "unknown command 'x'", Some("sync")),
        ];
        out.extend(m::COMMAND_HELP.iter().map(|h| command_help(st, h)));
        if st.tty {
            for h in [Home::NotConnected, Home::Unreadable, connected] {
                out.push(home(st, &h));
            }
        }
        out
    }

    #[test]
    fn no_screen_is_wider_than_80_columns_and_boxes_close() {
        for st in STYLES {
            for screen in every_screen(st) {
                for line in screen.lines() {
                    assert!(width(line) <= WIDTH, "{} columns: {line}", width(line));
                    let plain: String = line.chars().filter(|c| *c != '\x1b').collect();
                    if line.contains('│') || line.contains('╮') {
                        assert_eq!(width(line), WIDTH, "a box line ends short: {plain}");
                    }
                }
            }
        }
    }

    #[test]
    fn off_a_terminal_there_are_no_escapes_borders_or_owl() {
        let st = Style {
            color: false,
            tty: false,
        };
        for screen in every_screen(st) {
            assert!(!screen.contains('\x1b') && !screen.contains('│') && !screen.contains('─'));
        }
    }

    #[test]
    fn wrapping_keeps_words_whole_and_cuts_only_what_cannot_fit() {
        assert_eq!(wrap("a bb ccc dddd", 6), ["a bb", "ccc", "dddd"]);
        assert_eq!(wrap("abcdefgh x", 3), ["abc", "def", "gh", "x"]);
        assert_eq!(wrap("", 5), [""]);
    }

    #[test]
    fn every_typed_command_has_its_own_help() {
        for c in super::super::args::COMMANDS {
            assert!(m::command_help(c).is_some(), "{c}");
        }
    }

    #[test]
    fn the_next_step_follows_the_agents() {
        let st = Style {
            color: false,
            tty: true,
        };
        let next = |agents| {
            let h = Home::Connected {
                organization: "Acme".into(),
                agents,
                waiting: 0,
                last_sent: None,
            };
            home(st, &h)
        };
        assert!(
            next(vec![agent("Codex", true, false)]).contains("spendbranch hooks install codex")
        );
        assert!(next(vec![agent("Codex", false, false)]).contains("spendbranch setup"));
        assert!(next(vec![agent("Codex", true, true)]).contains("spendbranch status"));
    }
}
