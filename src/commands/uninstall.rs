//! `uninstall`: everything `setup` and the install script put on this computer, removed, one
//! step per line like `setup`. Each step runs even when one before it failed; the last line says
//! whether something is left.
use super::account::{self, Disconnected, Revoked};
use super::hooks::{self, Removal};
use super::{Ctx, Result, spinning};
use crate::cli::{messages as m, render, screen};
use crate::infra::{
    paths, scheduler,
    system::Platform,
    uninstall::{self as files, DataDir},
};
use crate::sources::Source;
use crate::uninstall as rules;

/// One step: how it ended and its line of detail.
type Step = fn(&mut Ctx) -> Result<(Mark, String)>;

/// How a step ended.
#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Ok,
    /// Done, but something is left for the person to do.
    Warn,
    Bad,
}

pub(super) fn run(ctx: &mut Ctx, yes: bool) -> Result<i32> {
    if !yes && !ctx.interactive {
        writeln!(ctx.out, "{}", m::UNINSTALL_NEEDS_YES)?;
        return Ok(2);
    }
    writeln!(ctx.out, "{}\n", screen::title(ctx.style, m::UNINSTALL_WHAT))?;
    if !yes {
        write!(ctx.out, "{}", m::UNINSTALL_ASK)?;
        ctx.out.flush()?;
        let mut answer = String::new();
        ctx.input.read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            writeln!(ctx.out, "{}", m::UNINSTALL_CANCELLED)?;
            return Ok(1);
        }
        writeln!(ctx.out)?;
    }
    let steps: [(&str, Step); 6] = [
        ("Disconnect", disconnect),
        ("Hooks", hooks),
        ("Schedule", schedule),
        (path_label(ctx), path_lines),
        ("Data", data),
        ("Program", program),
    ];
    let mut left = false;
    for (i, (label, run)) in steps.into_iter().enumerate() {
        let (mark, detail) = run(ctx)?;
        left |= mark != Mark::Ok;
        let st = ctx.style;
        let mark = match mark {
            Mark::Ok => st.ok(),
            Mark::Warn => st.warn(),
            Mark::Bad => st.bad(),
        };
        writeln!(ctx.out, "{}", screen::step(st, &mark, label, &detail))?;
        if i + 1 < steps.len()
            && let Some(joint) = screen::joint(st)
        {
            writeln!(ctx.out, "{joint}")?;
        }
    }
    writeln!(ctx.out)?;
    if left {
        writeln!(ctx.out, "{}", m::UNINSTALL_PARTIAL)?;
        Ok(1)
    } else {
        writeln!(ctx.out, "{}", m::UNINSTALL_DONE)?;
        Ok(0)
    }
}

fn path_label(ctx: &Ctx) -> &'static str {
    if ctx.system.platform() == Platform::Windows {
        "Path"
    } else {
        "PATH"
    }
}

/// A path for the screen: `~` for the home folder.
fn shown(ctx: &Ctx, path: &str) -> String {
    let home = if ctx.system.platform() == Platform::Windows {
        (ctx.env)("USERPROFILE")
    } else {
        (ctx.env)("HOME")
    };
    rules::tilde(path, home.as_deref())
}

/// Revokes the device when it is connected, a spinner turning while the server answers. Never
/// creates the data folder to find out.
fn disconnect(ctx: &mut Ctx) -> Result<(Mark, String)> {
    let exists = paths::data_dir(ctx.env).is_ok_and(|dir| files::exists(&dir));
    if !exists {
        return Ok((Mark::Ok, m::UNINSTALL_NOT_CONNECTED.into()));
    }
    let done = account::disconnect(ctx, &mut |ctx, request| {
        spinning(ctx, "Disconnect", m::UNINSTALL_REVOKING, request)
    })?;
    let Disconnected::Done { revoked, kept } = done else {
        return Ok((Mark::Ok, m::UNINSTALL_NOT_CONNECTED.into()));
    };
    if let Some(why) = kept.first() {
        return Ok((Mark::Warn, m::uninstall_key_kept(why)));
    }
    Ok(match revoked {
        Revoked::Yes => (Mark::Ok, m::UNINSTALL_REVOKED.into()),
        Revoked::Already => (Mark::Ok, m::UNINSTALL_ALREADY_REVOKED.into()),
        Revoked::No | Revoked::NotTried(_) => (Mark::Warn, m::UNINSTALL_NOT_REVOKED.into()),
    })
}

fn hooks(ctx: &mut Ctx) -> Result<(Mark, String)> {
    let mut removed = Vec::new();
    for source in [Source::ClaudeCode, Source::Codex] {
        match hooks::remove(ctx, source)? {
            Removal::Removed(_) => removed.push(render::agent_name(source)),
            Removal::Absent => {}
            Removal::Failed(why) => return Ok((Mark::Bad, why)),
        }
    }
    Ok(if removed.is_empty() {
        (Mark::Ok, m::UNINSTALL_NO_HOOKS.into())
    } else {
        (Mark::Ok, m::uninstall_hooks_removed(&removed.join(" · ")))
    })
}

fn schedule(ctx: &mut Ctx) -> Result<(Mark, String)> {
    Ok(
        match scheduler::remove(ctx.system, ctx.env, ctx.system.platform()) {
            Ok(true) => (Mark::Ok, m::UNINSTALL_SCHEDULE_REMOVED.into()),
            Ok(false) => (Mark::Ok, m::UNINSTALL_NO_SCHEDULE.into()),
            Err(why) => (Mark::Bad, m::scheduler_not_removed(&why)),
        },
    )
}

/// The PATH the install script set: the marked lines in the shell files, or on Windows the
/// program's folder in the user Path (only a folder named `spendbranch`).
fn path_lines(ctx: &mut Ctx) -> Result<(Mark, String)> {
    if ctx.system.platform() != Platform::Windows {
        let mut changed = Vec::new();
        for file in files::shell_files(ctx.env) {
            let name = shown(ctx, &file.display().to_string());
            match files::strip_path_lines(&file) {
                Ok(true) => changed.push(name),
                Ok(false) => {}
                Err(e) => return Ok((Mark::Bad, m::uninstall_failed(&name, &e.to_string()))),
            }
        }
        return Ok(if changed.is_empty() {
            (Mark::Ok, m::UNINSTALL_NOTHING.into())
        } else {
            (Mark::Ok, m::uninstall_path_files(&changed.join(", ")))
        });
    }
    let Some(dir) = ctx.system.exe().ok().and_then(|exe| {
        exe.to_str()
            .and_then(rules::windows_folder_of)
            .map(str::to_owned)
    }) else {
        return Ok((Mark::Ok, m::UNINSTALL_NOTHING.into()));
    };
    let script = rules::powershell_encoded(&rules::windows_path_script(&dir));
    let args = ["-NoProfile", "-NonInteractive", "-EncodedCommand", &script];
    Ok(match ctx.system.run("powershell", &args, None) {
        Ok(ran) if ran.code == 0 && ran.stdout.trim() == "removed" => {
            (Mark::Ok, m::uninstall_path_entry(&shown(ctx, &dir)))
        }
        Ok(ran) if ran.code == 0 => (Mark::Ok, m::UNINSTALL_NOTHING.into()),
        Ok(ran) => {
            let why = format!("{} {}", ran.stderr.trim(), ran.stdout.trim());
            (
                Mark::Bad,
                m::uninstall_failed(&shown(ctx, &dir), why.trim()),
            )
        }
        Err(e) => (
            Mark::Bad,
            m::uninstall_failed(&shown(ctx, &dir), &e.to_string()),
        ),
    })
}

fn data(ctx: &mut Ctx) -> Result<(Mark, String)> {
    let dir = match paths::data_dir(ctx.env) {
        Ok(dir) => dir,
        Err(e) => return Ok((Mark::Bad, e.to_string())),
    };
    let name = shown(ctx, &dir.display().to_string());
    Ok(match files::remove_data_dir(&dir) {
        Ok(DataDir::Removed) => (Mark::Ok, m::uninstall_deleted(&name)),
        Ok(DataDir::Absent) => (Mark::Ok, m::UNINSTALL_NOTHING.into()),
        Ok(DataDir::NotOurs) => (Mark::Warn, m::uninstall_data_not_ours(&name)),
        Err(e) => (Mark::Bad, m::uninstall_failed(&name, &e.to_string())),
    })
}

/// Last, so every step before it could still run from this file.
fn program(ctx: &mut Ctx) -> Result<(Mark, String)> {
    let exe = match ctx.system.exe() {
        Ok(exe) => exe,
        Err(e) => return Ok((Mark::Bad, e.to_string())),
    };
    let path = exe.display().to_string();
    if rules::installed_by_homebrew(&path) {
        return Ok((Mark::Warn, m::UNINSTALL_HOMEBREW.into()));
    }
    let name = shown(ctx, &path);
    Ok(match ctx.system.remove_program(&exe) {
        Ok(()) => (Mark::Ok, m::uninstall_deleted(&name)),
        Err(e) => (Mark::Bad, m::uninstall_failed(&name, &e.to_string())),
    })
}
