//! `hooks install|uninstall`: edit the agent's own hook file, and read how it stands.
use super::{Ctx, Result, say};
use crate::cli::{messages as m, render};
use crate::contract::Source;
use crate::env::agent_dir;
use crate::hooks::{self as rules, Plan, Problem, State};
use crate::infra::{agent_files, hook_log, paths, system::Platform};
use std::{io, path::PathBuf};

fn hook_file(ctx: &Ctx, source: Source) -> Option<PathBuf> {
    let name = match source {
        Source::ClaudeCode => "settings.json",
        Source::Codex => "hooks.json",
    };
    agent_dir(ctx.env, source).map(|d| d.join(name))
}

/// Hook lines are quoted for the shell of the system the agent runs on.
fn windows(ctx: &Ctx) -> bool {
    ctx.system.platform() == Platform::Windows
}

/// The absolute path of this program, as text (the hook and schedule lines are text).
pub(super) fn exe_text(ctx: &Ctx) -> Result<String> {
    let path = ctx.system.exe()?;
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "the path of this program is not valid text".into())
}

/// What the agent's file holds, for the rules to edit.
enum Read {
    /// Its text, or `None` when there is no file.
    Text(Option<String>),
    /// It exists but is not text we can read as JSON.
    Unusable,
}

fn read(file: &std::path::Path) -> Result<Read> {
    match agent_files::read(file) {
        Ok(text) => Ok(Read::Text(text)),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => Ok(Read::Unusable),
        Err(e) => Err(e.into()),
    }
}

pub(super) fn install(ctx: &mut Ctx, source: Source) -> Result<i32> {
    let agent = render::agent_name(source);
    let Some(file) = hook_file(ctx, source) else {
        return say(ctx, m::NO_HOME, 1);
    };
    let shown = file.display().to_string();
    let Read::Text(original) = read(&file)? else {
        return say(ctx, &m::hook_file_problem(&shown, false), 1);
    };
    let plan = match rules::install(source, original.as_deref(), &exe_text(ctx)?, windows(ctx)) {
        Ok(plan) => plan,
        Err(problem) => {
            return say(
                ctx,
                &m::hook_file_problem(&shown, problem == Problem::Layout),
                1,
            );
        }
    };
    match plan {
        Plan::Unchanged => {
            say(ctx, &m::hook_already(agent), 0)?;
        }
        Plan::Write(text) => {
            agent_files::write(&file, &text)?;
            say(ctx, &m::hook_installed(agent, &shown), 0)?;
            if (ctx.env)("SPENDBRANCH_HOME").is_some_and(|h| !h.is_empty()) {
                say(ctx, m::HOME_NOT_CARRIED, 0)?;
            }
        }
    }
    if source == Source::Codex {
        say(ctx, m::CODEX_TRUST, 0)?;
    }
    Ok(0)
}

pub(super) fn uninstall(ctx: &mut Ctx, source: Source) -> Result<i32> {
    let agent = render::agent_name(source);
    let Some(file) = hook_file(ctx, source) else {
        return say(ctx, m::NO_HOME, 1);
    };
    let shown = file.display().to_string();
    let Read::Text(original) = read(&file)? else {
        return say(ctx, &m::hook_file_problem(&shown, false), 1);
    };
    match rules::uninstall(source, original.as_deref()) {
        Err(problem) => say(
            ctx,
            &m::hook_file_problem(&shown, problem == Problem::Layout),
            1,
        ),
        Ok(Plan::Unchanged) => say(ctx, &m::hook_absent(agent), 0),
        Ok(Plan::Write(text)) => {
            agent_files::write(&file, &text)?;
            say(ctx, &m::hook_removed(agent, &shown), 0)
        }
    }
}

/// How the hook stands, for `status` and the hint in `enable`. A file that cannot be read at
/// all is as unusable as one that is not JSON: `status` must not fail over it.
pub(super) fn state(ctx: &Ctx, source: Source) -> State {
    let Some(file) = hook_file(ctx, source) else {
        return State::Invalid;
    };
    let Ok(Read::Text(original)) = read(&file) else {
        return State::Invalid;
    };
    let exe = exe_text(ctx).unwrap_or_default();
    rules::state(source, original.as_deref(), &exe, windows(ctx))
}

/// The state plus, for Codex, when the hook last ran (Codex ignores hooks it does not trust).
pub(super) fn view(ctx: &Ctx, source: Source) -> render::HookView {
    let last_firing = (source == Source::Codex)
        .then(|| {
            let dir = paths::data_dir(ctx.env).ok()?;
            rules::last_firing(&hook_log::read(&dir)?, "codex:")
        })
        .flatten();
    render::HookView {
        state: state(ctx, source),
        last_firing,
    }
}
