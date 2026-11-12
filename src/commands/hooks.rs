//! `hooks install|uninstall`: edit the agent's own hook file, and read how it stands.
use super::{Ctx, Result, say};
use crate::cli::{messages as m, render};
use crate::contract::Source;
use crate::hooks::{self as rules, Plan, Problem, State};
use crate::infra::{agent_files, paths};
use std::{fs, io, path::PathBuf};

fn home(ctx: &Ctx) -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    (ctx.env)(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The agent's configuration folder: `$CLAUDE_CONFIG_DIR` / `~/.claude`, `$CODEX_HOME` / `~/.codex`.
pub(super) fn agent_dir(ctx: &Ctx, source: Source) -> Option<PathBuf> {
    let (var, dot) = match source {
        Source::ClaudeCode => ("CLAUDE_CONFIG_DIR", ".claude"),
        Source::Codex => ("CODEX_HOME", ".codex"),
    };
    match (ctx.env)(var).filter(|d| !d.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => home(ctx).map(|h| h.join(dot)),
    }
}

fn hook_file(ctx: &Ctx, source: Source) -> Option<PathBuf> {
    let name = match source {
        Source::ClaudeCode => "settings.json",
        Source::Codex => "hooks.json",
    };
    agent_dir(ctx, source).map(|d| d.join(name))
}

pub(super) fn arg_of(source: Source) -> &'static str {
    match source {
        Source::ClaudeCode => "claude-code",
        Source::Codex => "codex",
    }
}

fn exe(ctx: &Ctx) -> Result<String> {
    let path = ctx.system.exe()?;
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "the path of this program is not valid text".into())
}

/// The file's text; `Err(text)` when it cannot be used as JSON text at all.
fn read(file: &std::path::Path) -> Result<std::result::Result<Option<String>, ()>> {
    match agent_files::read(file) {
        Ok(text) => Ok(Ok(text)),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => Ok(Err(())),
        Err(e) => Err(e.into()),
    }
}

pub(super) fn install(ctx: &mut Ctx, source: Source) -> Result<i32> {
    let agent = render::agent_name(source);
    let Some(file) = hook_file(ctx, source) else {
        return say(ctx, m::NO_HOME, 1);
    };
    let shown = file.display().to_string();
    let Ok(original) = read(&file)? else {
        return say(ctx, &m::hook_file_problem(&shown, false), 1);
    };
    let plan = match rules::install(source, original.as_deref(), &exe(ctx)?, cfg!(windows)) {
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
    let Ok(original) = read(&file)? else {
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

/// How the hook stands, for `status` and the hint in `enable`.
pub(super) fn state(ctx: &Ctx, source: Source) -> State {
    let Some(file) = hook_file(ctx, source) else {
        return State::Invalid;
    };
    let Ok(original) = read(&file).unwrap_or(Err(())) else {
        return State::Invalid;
    };
    let exe = ctx
        .system
        .exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_default();
    rules::state(source, original.as_deref(), &exe, cfg!(windows))
}

/// The state plus, for Codex, when the hook last ran (Codex ignores hooks it does not trust).
pub(super) fn view(ctx: &Ctx, source: Source) -> render::HookView {
    let last_firing = (source == Source::Codex)
        .then(|| {
            let dir = paths::data_dir(ctx.env).ok()?;
            let log = fs::read_to_string(dir.join("hook.log")).ok()?;
            rules::last_firing(&log, "codex:")
        })
        .flatten();
    render::HookView {
        state: state(ctx, source),
        last_firing,
    }
}
