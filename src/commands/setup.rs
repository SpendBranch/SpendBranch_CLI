//! `setup`: connect, find the agents, turn collection on, install the hooks and the schedule.
//! Running it again changes nothing that is already right.
use super::{
    Ctx, Result,
    account::{self, Loaded},
    hooks, say,
};
use crate::api::ServerUrl;
use crate::cli::{messages as m, render};
use crate::contract::Source;
use crate::infra::scheduler::{self, Platform};

const AGENTS: [Source; 2] = [Source::ClaudeCode, Source::Codex];

pub(super) fn run(
    ctx: &mut Ctx,
    server: Option<String>,
    only: Option<Source>,
    relogin: bool,
) -> Result<i32> {
    let code = connect(ctx, server, relogin)?;
    if code != 0 {
        return Ok(code);
    }
    // Detect: an agent is there when its configuration folder is.
    let mut found = Vec::new();
    for source in AGENTS.into_iter().filter(|s| only.is_none_or(|o| o == *s)) {
        let there = hooks::agent_dir(ctx, source).is_some_and(|d| d.is_dir());
        say(ctx, &m::agent_found(render::agent_name(source), there), 0)?;
        if there {
            found.push(source);
        }
    }
    if found.is_empty() {
        return say(ctx, m::SETUP_NO_AGENT, 1);
    }
    let mut parts = Vec::new();
    let mut all_on = true;
    for source in found {
        let agent = render::agent_name(source);
        let on = account::enable_with(ctx, source, false)? == 0;
        let hooked = on && hooks::install(ctx, source)? == 0;
        all_on &= hooked;
        parts.push(format!(
            "{agent}: {}",
            match (on, hooked) {
                (true, true) => "connected, with the hook",
                (true, false) => "connected, without the hook",
                _ => "not connected",
            }
        ));
    }
    let scheduled = match ctx.system.exe().map_err(|e| e.to_string()).and_then(|p| {
        let exe = p.to_str().map(str::to_owned);
        exe.ok_or_else(|| "the path of this program is not valid text".to_owned())
    }) {
        Ok(exe) => scheduler::install(ctx.system, ctx.env, Platform::current(), &exe),
        Err(why) => Err(why),
    };
    match &scheduled {
        Ok(method) => say(ctx, &m::scheduler_installed(method.describe()), 0)?,
        Err(why) => say(ctx, &m::scheduler_failed(why), 0)?,
    };
    say(ctx, &format!("Summary: {}.", parts.join("; ")), 0)?;
    Ok(i32::from(!(all_on && scheduled.is_ok())))
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
    account::login(ctx, explicit.or(saved))
}
