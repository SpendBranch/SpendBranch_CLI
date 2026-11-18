//! `uninstall`: everything `setup` and the install script put on this computer, removed. Each
//! step runs even when one before it failed; the last line says whether something is left.
use super::{Ctx, Result, account, hooks, say};
use crate::cli::messages as m;
use crate::infra::{
    paths, scheduler,
    system::Platform,
    uninstall::{self as files, DataDir},
};
use crate::sources::Source;
use crate::uninstall as rules;

pub(super) fn run(ctx: &mut Ctx, yes: bool) -> Result<i32> {
    if !yes {
        if !ctx.interactive {
            return say(ctx, m::UNINSTALL_NEEDS_YES, 2);
        }
        write!(ctx.out, "{}", m::UNINSTALL_ASK)?;
        ctx.out.flush()?;
        let mut answer = String::new();
        ctx.input.read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return say(ctx, m::UNINSTALL_CANCELLED, 1);
        }
    }
    let mut left = disconnect(ctx)?;
    for source in [Source::ClaudeCode, Source::Codex] {
        left |= hooks::uninstall(ctx, source)? != 0;
    }
    left |= path_lines(ctx)?;
    left |= data(ctx)?;
    left |= program(ctx)?;
    if left {
        say(ctx, m::UNINSTALL_PARTIAL, 1)
    } else {
        say(ctx, m::UNINSTALL_DONE, 0)
    }
}

/// Revokes the device when it is connected (`logout`, which also removes the schedule), and
/// otherwise removes the schedule alone. Never creates the data folder to find out.
fn disconnect(ctx: &mut Ctx) -> Result<bool> {
    let connected = match paths::data_dir(ctx.env) {
        Ok(dir) if files::exists(&dir) => account::open(ctx)?.store.is_connected()?,
        _ => false,
    };
    if connected {
        return Ok(account::logout(ctx)? != 0);
    }
    match scheduler::remove(ctx.system, ctx.env, ctx.system.platform()) {
        Ok(true) => say(ctx, m::SCHEDULER_REMOVED, 0).map(|_| false),
        Ok(false) => Ok(false),
        Err(why) => say(ctx, &m::scheduler_not_removed(&why), 0).map(|_| true),
    }
}

/// The PATH the install script set: the marked lines in the shell files, or on Windows the
/// program's folder in the user Path (only a folder named `spendbranch`).
fn path_lines(ctx: &mut Ctx) -> Result<bool> {
    if ctx.system.platform() != Platform::Windows {
        let mut left = false;
        for file in files::shell_files(ctx.env) {
            let shown = file.display().to_string();
            match files::strip_path_lines(&file) {
                Ok(true) => {
                    say(ctx, &m::uninstall_path_line(&shown), 0)?;
                }
                Ok(false) => {}
                Err(e) => {
                    say(ctx, &m::uninstall_failed(&shown, &e.to_string()), 0)?;
                    left = true;
                }
            }
        }
        return Ok(left);
    }
    let Ok(exe) = ctx.system.exe() else {
        return Ok(false);
    };
    let Some(dir) = exe.to_str().and_then(rules::windows_folder_of) else {
        return Ok(false);
    };
    let shown = dir.to_owned();
    let script = rules::powershell_encoded(&rules::windows_path_script(&shown));
    let args = ["-NoProfile", "-NonInteractive", "-EncodedCommand", &script];
    match ctx.system.run("powershell", &args, None) {
        Ok(ran) if ran.code == 0 => {
            if ran.stdout.trim() == "removed" {
                say(ctx, &m::uninstall_path_entry(&shown), 0)?;
            }
            Ok(false)
        }
        Ok(ran) => {
            let why = format!("{} {}", ran.stderr.trim(), ran.stdout.trim());
            say(
                ctx,
                &m::uninstall_failed("it from your user Path", why.trim()),
                0,
            )?;
            Ok(true)
        }
        Err(e) => {
            say(
                ctx,
                &m::uninstall_failed("it from your user Path", &e.to_string()),
                0,
            )?;
            Ok(true)
        }
    }
}

fn data(ctx: &mut Ctx) -> Result<bool> {
    let dir = match paths::data_dir(ctx.env) {
        Ok(dir) => dir,
        Err(e) => {
            say(
                ctx,
                &m::uninstall_failed("the data folder", &e.to_string()),
                0,
            )?;
            return Ok(true);
        }
    };
    let shown = dir.display().to_string();
    match files::remove_data_dir(&dir) {
        Ok(DataDir::Removed) => say(ctx, &m::uninstall_data_removed(&shown), 0).map(|_| false),
        Ok(DataDir::Absent) => Ok(false),
        Ok(DataDir::NotOurs) => say(ctx, &m::uninstall_data_not_ours(&shown), 0).map(|_| true),
        Err(e) => say(ctx, &m::uninstall_failed(&shown, &e.to_string()), 0).map(|_| true),
    }
}

/// Last, so every step before it could still run from this file.
fn program(ctx: &mut Ctx) -> Result<bool> {
    let exe = match ctx.system.exe() {
        Ok(exe) => exe,
        Err(e) => {
            say(ctx, &m::uninstall_failed("the program", &e.to_string()), 0)?;
            return Ok(true);
        }
    };
    let shown = exe.display().to_string();
    if rules::installed_by_homebrew(&shown) {
        return say(ctx, m::UNINSTALL_HOMEBREW, 0).map(|_| true);
    }
    match ctx.system.remove_program(&exe) {
        Ok(()) => say(ctx, &m::uninstall_program_removed(&shown), 0).map(|_| false),
        Err(e) => say(ctx, &m::uninstall_failed(&shown, &e.to_string()), 0).map(|_| true),
    }
}
