//! The processes: our own path, the agent's stdin, the detached `sync`, and the system tools
//! (`systemctl`, `crontab`, `launchctl`, `schtasks`). A trait, so tests run none of them.
use super::private_fs;
use std::{
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

/// How long the hook waits for the agent's stdin.
const STDIN_WAIT: Duration = Duration::from_millis(100);

/// What a finished tool said.
pub struct Ran {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait System {
    /// The absolute path of the running executable.
    fn exe(&self) -> io::Result<PathBuf>;
    /// Up to 64 KiB of stdin, if it arrives within 100 ms. Nothing for a terminal.
    fn stdin_brief(&self) -> Option<Vec<u8>>;
    /// Starts `exe args` and does not wait: it outlives us, its output goes to `log`.
    fn spawn_detached(&self, exe: &Path, args: &[&str], log: &Path) -> io::Result<()>;
    /// Runs a tool to the end.
    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> io::Result<Ran>;
    /// A long-running command (`relay`) should end now. The real answer is never: it ends with
    /// the signal (Ctrl+C), which holds nothing that needs closing.
    fn stopped(&self) -> bool;
}

pub struct Real;

impl System for Real {
    fn exe(&self) -> io::Result<PathBuf> {
        std::env::current_exe()
    }

    fn stdin_brief(&self) -> Option<Vec<u8>> {
        if io::stdin().is_terminal() {
            return None;
        }
        // The read runs on its own thread: a pipe that never closes must not hold the agent.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let read = io::stdin()
                .lock()
                .take(crate::hooks::MAX_INPUT as u64)
                .read_to_end(&mut buf);
            let _ = tx.send(read.ok().map(|_| buf));
        });
        rx.recv_timeout(STDIN_WAIT).ok().flatten()
    }

    fn spawn_detached(&self, exe: &Path, args: &[&str], log: &Path) -> io::Result<()> {
        let out = private_fs::open(log, true)?;
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::from(out.try_clone()?))
            .stdout(Stdio::from(out));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0); // the agent killing its own group does not kill the sync
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        }
        cmd.spawn().map(drop)
    }

    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> io::Result<Ran> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(text.as_bytes())?; // dropped here: the tool sees the end of input
        }
        let out = child.wait_with_output()?;
        Ok(Ran {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn stopped(&self) -> bool {
        false
    }
}

/// For `cli::run`, which must never start anything.
pub struct Inert;

impl System for Inert {
    fn exe(&self) -> io::Result<PathBuf> {
        Err(io::Error::other("no executable path in this mode"))
    }
    fn stdin_brief(&self) -> Option<Vec<u8>> {
        None
    }
    fn spawn_detached(&self, _: &Path, _: &[&str], _: &Path) -> io::Result<()> {
        Ok(())
    }
    fn run(&self, _: &str, _: &[&str], _: Option<&str>) -> io::Result<Ran> {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no tools in this mode",
        ))
    }
    fn stopped(&self) -> bool {
        true
    }
}
