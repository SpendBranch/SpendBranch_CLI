//! Installs and removes the system's periodic `sync`: a systemd user timer (or the crontab
//! where there is no systemd), a LaunchAgent, or Task Scheduler tasks. Idempotent.
use super::system::System;
use crate::env::{Env, user_home};
use crate::scheduler::{self as text, Job};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// What ended up scheduling the sync.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Systemd,
    Cron,
    Launchd,
    TaskScheduler,
}

impl Method {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Systemd => "systemd user timer",
            Self::Cron => "crontab",
            Self::Launchd => "LaunchAgent",
            Self::TaskScheduler => "Task Scheduler",
        }
    }
}

type Result<T> = std::result::Result<T, String>;

fn home(env: Env) -> Result<PathBuf> {
    user_home(env).ok_or_else(|| "cannot locate the home folder".to_owned())
}

fn systemd_dir(env: Env) -> Result<PathBuf> {
    let xdg = env("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    let base = match xdg {
        Some(x) => x,
        None => home(env)?.join(".config"),
    };
    Ok(base.join("systemd").join("user"))
}

fn plist_path(env: Env) -> Result<PathBuf> {
    Ok(home(env)?
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", text::LAUNCHD_LABEL)))
}

fn tool(sys: &dyn System, program: &str, args: &[&str], stdin: Option<&str>) -> Result<()> {
    let ran = sys
        .run(program, args, stdin)
        .map_err(|e| format!("{program}: {e}"))?;
    if ran.code == 0 {
        return Ok(());
    }
    let said = ran.stderr.trim();
    Err(if said.is_empty() {
        format!("{program} exited with {}", ran.code)
    } else {
        format!("{program}: {said}")
    })
}

fn write(path: &Path, content: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))
}

/// The crontab as it is, or empty. `None` when there is no `crontab` tool at all.
fn crontab(sys: &dyn System) -> Option<String> {
    let ran = sys.run("crontab", &["-l"], None).ok()?;
    // No crontab yet exits non-zero: that is an empty one.
    Some(if ran.code == 0 {
        ran.stdout
    } else {
        String::new()
    })
}

fn systemd_usable(sys: &dyn System) -> bool {
    tool(sys, "systemctl", &["--user", "show-environment"], None).is_ok()
}

pub fn install(sys: &dyn System, env: Env, platform: Platform, exe: &str) -> Result<Method> {
    let home_dir = env("SPENDBRANCH_HOME").filter(|h| !h.is_empty());
    let job = Job {
        exe,
        home: home_dir.as_deref(),
    };
    match platform {
        Platform::Linux if systemd_usable(sys) => install_systemd(sys, env, &job),
        Platform::Linux => install_cron(sys, &job),
        Platform::MacOs => install_launchd(sys, env, &job),
        Platform::Windows => install_tasks(sys, &job),
    }
}

fn install_systemd(sys: &dyn System, env: Env, job: &Job) -> Result<Method> {
    let dir = systemd_dir(env)?;
    let timer = format!("{}.timer", text::SYSTEMD_UNIT);
    write(
        &dir.join(format!("{}.service", text::SYSTEMD_UNIT)),
        &text::systemd_service(job),
    )?;
    write(&dir.join(&timer), &text::systemd_timer())?;
    tool(sys, "systemctl", &["--user", "daemon-reload"], None)?;
    tool(
        sys,
        "systemctl",
        &["--user", "enable", "--now", &timer],
        None,
    )?;
    Ok(Method::Systemd)
}

fn install_cron(sys: &dyn System, job: &Job) -> Result<Method> {
    let current = crontab(sys)
        .ok_or("neither `systemctl --user` nor `crontab` is available on this system")?;
    tool(
        sys,
        "crontab",
        &["-"],
        Some(&text::cron_merge(&current, job)),
    )?;
    Ok(Method::Cron)
}

fn install_launchd(sys: &dyn System, env: Env, job: &Job) -> Result<Method> {
    let path = plist_path(env)?;
    let file = path.to_string_lossy().into_owned();
    write(&path, &text::launchd_plist(job))?;
    let _ = tool(sys, "launchctl", &["unload", &file], None); // not loaded yet is fine
    tool(sys, "launchctl", &["load", "-w", &file], None)?;
    Ok(Method::Launchd)
}

fn install_tasks(sys: &dyn System, job: &Job) -> Result<Method> {
    for args in text::schtasks_create(job) {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        tool(sys, "schtasks", &args, None)?;
    }
    Ok(Method::TaskScheduler)
}

/// `Ok(true)` when something was installed and is now gone.
pub fn remove(sys: &dyn System, env: Env, platform: Platform) -> Result<bool> {
    match platform {
        Platform::Linux => {
            let systemd = remove_systemd(sys, env)?;
            Ok(remove_cron(sys)? | systemd)
        }
        Platform::MacOs => remove_launchd(sys, env),
        Platform::Windows => Ok(text::WINDOWS_TASKS.iter().fold(false, |was, name| {
            was | tool(sys, "schtasks", &["/Delete", "/TN", name, "/F"], None).is_ok()
        })),
    }
}

fn remove_systemd(sys: &dyn System, env: Env) -> Result<bool> {
    let dir = systemd_dir(env)?;
    let name = format!("{}.timer", text::SYSTEMD_UNIT);
    let files = [
        dir.join(&name),
        dir.join(format!("{}.service", text::SYSTEMD_UNIT)),
    ];
    if !files.iter().any(|f| f.exists()) {
        return Ok(false);
    }
    let _ = tool(
        sys,
        "systemctl",
        &["--user", "disable", "--now", &name],
        None,
    );
    for file in &files {
        match fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("{}: {e}", file.display()));
            }
            _ => {}
        }
    }
    let _ = tool(sys, "systemctl", &["--user", "daemon-reload"], None);
    Ok(true)
}

fn remove_cron(sys: &dyn System) -> Result<bool> {
    let Some(current) = crontab(sys).filter(|c| text::has_cron(c)) else {
        return Ok(false);
    };
    tool(sys, "crontab", &["-"], Some(&text::cron_remove(&current)))?;
    Ok(true)
}

fn remove_launchd(sys: &dyn System, env: Env) -> Result<bool> {
    let path = plist_path(env)?;
    if !path.exists() {
        return Ok(false);
    }
    let _ = tool(sys, "launchctl", &["unload", &path.to_string_lossy()], None);
    fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

pub fn installed(sys: &dyn System, env: Env, platform: Platform) -> bool {
    match platform {
        Platform::Linux => {
            systemd_dir(env).is_ok_and(|d| d.join(format!("{}.timer", text::SYSTEMD_UNIT)).exists())
                || crontab(sys).is_some_and(|c| text::has_cron(&c))
        }
        Platform::MacOs => plist_path(env).is_ok_and(|p| p.exists()),
        Platform::Windows => tool(
            sys,
            "schtasks",
            &["/Query", "/TN", text::WINDOWS_TASKS[0]],
            None,
        )
        .is_ok(),
    }
}
