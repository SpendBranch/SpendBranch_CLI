//! `setup`: connect, find the agents, turn collection on, install the hooks and the schedule.
//! Running it again changes nothing that is already right. Each step shows how it ended, with
//! what it said nested under it.
use super::{
    Ctx, Result,
    account::{self, Loaded},
    hooks, say,
};
use crate::api::ServerUrl;
use crate::cli::{messages as m, render, screen};
use crate::contract::Source;
use crate::env::agent_dir;
use crate::infra::scheduler;
use std::io::{self, Write};

const AGENTS: [Source; 2] = [Source::ClaudeCode, Source::Codex];

/// How a step ended.
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Ok,
    Warn,
    Bad,
}

impl Mark {
    /// All of `done` out of `total`.
    fn of(done: usize, total: usize) -> Self {
        match done {
            0 => Self::Bad,
            d if d == total => Self::Ok,
            _ => Self::Warn,
        }
    }
}

pub(super) fn run(
    ctx: &mut Ctx,
    server: Option<String>,
    only: Option<Source>,
    relogin: bool,
) -> Result<i32> {
    // The login talks to the person (a code, a question), so its lines show as they come.
    say(ctx, &screen::step_start(ctx.style, "Connect"), 0)?;
    let code = live(ctx, |ctx| connect(ctx, server, relogin))?;
    if code != 0 {
        step(ctx, Mark::Bad, "Connect", "", "")?;
        return Ok(code);
    }
    let organization = account::open(ctx)?.store.meta_get("org_name")?;
    step(
        ctx,
        Mark::Ok,
        "Connect",
        &organization.unwrap_or_default(),
        "",
    )?;

    let asked: Vec<Source> = AGENTS
        .into_iter()
        .filter(|s| only.is_none_or(|o| o == *s))
        .collect();
    let found: Vec<Source> = asked
        .iter()
        .copied()
        .filter(|s| agent_dir(ctx.env, *s).is_some_and(|d| d.is_dir()))
        .collect();
    let seen: Vec<String> = asked
        .iter()
        .map(|s| {
            let there = if found.contains(s) {
                "found"
            } else {
                "not found"
            };
            format!("{} {there}", render::agent_name(*s))
        })
        .collect();
    if found.is_empty() {
        step(
            ctx,
            Mark::Bad,
            "Agents",
            &seen.join(" · "),
            m::SETUP_NO_AGENT,
        )?;
        return Ok(1);
    }
    step(ctx, Mark::Ok, "Agents", &seen.join(" · "), "")?;

    let (on, said) = quiet(ctx, |ctx| {
        let mut on = Vec::new();
        for &source in &found {
            if account::enable_with(ctx, source, false)? == 0 {
                on.push(source);
            }
        }
        Ok(on)
    })?;
    step(
        ctx,
        Mark::of(on.len(), found.len()),
        "Reading",
        &names(&on, &found),
        &said,
    )?;

    let (hooked, said) = quiet(ctx, |ctx| {
        let mut hooked = Vec::new();
        for &source in &on {
            if hooks::install(ctx, source)? == 0 {
                hooked.push(source);
            }
        }
        Ok(hooked)
    })?;
    step(
        ctx,
        Mark::of(hooked.len(), found.len()),
        "Hooks",
        &names(&hooked, &found),
        &said,
    )?;

    let scheduled = hooks::exe_text(ctx)
        .map_err(|e| e.to_string())
        .and_then(|exe| scheduler::install(ctx.system, ctx.env, ctx.system.platform(), &exe));
    match &scheduled {
        Ok(method) => step(
            ctx,
            Mark::Ok,
            "Schedule",
            &m::schedule_every(method.describe()),
            "",
        )?,
        Err(why) => step(
            ctx,
            Mark::Warn,
            "Schedule",
            "not installed",
            &m::scheduler_failed(why),
        )?,
    };
    let all = hooked.len() == found.len() && scheduled.is_ok();
    writeln!(ctx.out)?;
    say(
        ctx,
        if all {
            m::SETUP_READY
        } else {
            m::SETUP_PROBLEMS
        },
        i32::from(!all),
    )
}

/// "Claude Code · Codex off": the agents a step got to, and the ones it did not.
fn names(done: &[Source], all: &[Source]) -> String {
    let parts: Vec<String> = all
        .iter()
        .map(|s| {
            let name = render::agent_name(*s);
            if done.contains(s) {
                name.to_owned()
            } else {
                format!("{name} off")
            }
        })
        .collect();
    parts.join(" · ")
}

/// A step's line, then what it said nested under it (wrapped to the screen), then the joint
/// to the next step.
fn step(ctx: &mut Ctx, mark: Mark, label: &str, detail: &str, said: &str) -> Result<()> {
    let st = ctx.style;
    let mark = match mark {
        Mark::Ok => st.ok(),
        Mark::Warn => st.warn(),
        Mark::Bad => st.bad(),
    };
    writeln!(ctx.out, "{}", screen::step(st, &mark, label, detail))?;
    let nest = screen::nest(st);
    for line in said.lines() {
        let parts = if st.tty {
            screen::wrap(line, screen::WIDTH - 4)
        } else {
            vec![line.to_owned()]
        };
        for part in parts {
            writeln!(ctx.out, "{nest}{part}")?;
        }
    }
    if label != "Schedule"
        && let Some(joint) = screen::joint(st)
    {
        writeln!(ctx.out, "{joint}")?;
    }
    Ok(())
}

/// Runs `f` with its output nested as it is written.
fn live<T>(ctx: &mut Ctx, f: impl FnOnce(&mut Ctx) -> Result<T>) -> Result<T> {
    let prefix = screen::nest(ctx.style);
    let mut nested = Nested {
        out: &mut *ctx.out,
        prefix: &prefix,
        fresh: true,
    };
    // Field by field: `nested` holds `ctx.out`, the rest is borrowed beside it.
    f(&mut Ctx {
        env: ctx.env,
        out: &mut nested,
        input: &mut *ctx.input,
        sleep: ctx.sleep,
        open_url: ctx.open_url,
        interactive: ctx.interactive,
        system: ctx.system,
        style: ctx.style,
    })
}

/// Runs `f` and hands back what it printed, to show after the step's line.
fn quiet<T>(ctx: &mut Ctx, f: impl FnOnce(&mut Ctx) -> Result<T>) -> Result<(T, String)> {
    let mut buffer = Vec::new();
    let value = f(&mut with_out(ctx.env, &mut buffer, ctx))?;
    Ok((value, String::from_utf8_lossy(&buffer).into_owned()))
}

/// The same context, writing to `out`.
fn with_out<'b>(env: crate::env::Env<'b>, out: &'b mut dyn Write, ctx: &'b mut Ctx) -> Ctx<'b> {
    Ctx {
        env,
        out,
        input: &mut *ctx.input,
        sleep: ctx.sleep,
        open_url: ctx.open_url,
        interactive: ctx.interactive,
        system: ctx.system,
        style: ctx.style,
    }
}

/// Puts `prefix` before every line written through it; a prompt without its line end still
/// shows (flush passes through).
struct Nested<'a> {
    out: &'a mut dyn Write,
    prefix: &'a str,
    fresh: bool,
}

impl Write for Nested<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for line in buf.split_inclusive(|b| *b == b'\n') {
            if self.fresh {
                self.out.write_all(self.prefix.as_bytes())?;
            }
            self.out.write_all(line)?;
            self.fresh = line.ends_with(b"\n");
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// The device ends up paired: already, or after a login.
fn connect(ctx: &mut Ctx, server: Option<String>, relogin: bool) -> Result<i32> {
    let explicit = server.or_else(|| (ctx.env)("SPENDBRANCH_SERVER").filter(|s| !s.is_empty()));
    let account = account::open(ctx)?;
    let saved = account.store.meta_get("server")?;
    if !relogin {
        match account::load(&account)? {
            Loaded::Ready(c) => {
                let differs = explicit
                    .as_deref()
                    .and_then(|t| ServerUrl::parse(t).ok())
                    .is_some_and(|s| s.base() != c.server.base());
                if differs {
                    let wanted = explicit.unwrap_or_default();
                    return say(ctx, &m::setup_server_differs(&c.server.base(), &wanted), 1);
                }
                match account::status_of(&c) {
                    Ok(view) => {
                        return say(ctx, &m::setup_already(&view.organization_name), 0);
                    }
                    Err(f) if f.code == "UNAUTHENTICATED" => {
                        say(ctx, m::SETUP_RECONNECT, 0)?;
                    }
                    Err(f) => {
                        return say(ctx, &m::failure_text(&f, account::now().millis()), 1);
                    }
                }
            }
            Loaded::Broken(text) => {
                say(ctx, &text, 0)?;
            }
            Loaded::NotConnected => {}
        }
    }
    drop(account);
    account::login_with(ctx, explicit.or(saved), false)
}
