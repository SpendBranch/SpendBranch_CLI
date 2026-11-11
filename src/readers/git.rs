//! Turns a project folder into `host/owner/name` by asking git for its `origin`.
//! No shell, a short timeout, and the folder itself never leaves this module.
use crate::sources::repository::repository_key_of;
use std::{
    collections::HashMap,
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub struct RepoResolver {
    program: OsString,
    home: Option<PathBuf>,
    timeout: Duration,
    cache: HashMap<String, Option<String>>,
}

impl RepoResolver {
    pub fn new(home: Option<PathBuf>) -> Self {
        Self::with_program("git".into(), home, Duration::from_secs(2))
    }

    pub fn with_program(program: OsString, home: Option<PathBuf>, timeout: Duration) -> Self {
        Self {
            program,
            home,
            timeout,
            cache: HashMap::new(),
        }
    }

    /// `None` for the home folder, a filesystem root, a relative path, a folder with no
    /// `origin`, or git failing or taking too long. Answers are cached per folder.
    pub fn resolve(&mut self, cwd: &str) -> Option<String> {
        if let Some(known) = self.cache.get(cwd) {
            return known.clone();
        }
        let found = self
            .is_project_folder(cwd)
            .then(|| self.ask_git(cwd))
            .flatten();
        self.cache.insert(cwd.to_owned(), found.clone());
        found
    }

    fn is_project_folder(&self, cwd: &str) -> bool {
        let path = Path::new(cwd);
        path.is_absolute() && path.parent().is_some() && self.home.as_deref() != Some(path)
    }

    fn ask_git(&self, cwd: &str) -> Option<String> {
        let mut command = Command::new(&self.program);
        command
            .args(["-C", cwd, "remote", "get-url", "origin"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().ok()?;
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait().ok()? {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                None => thread::sleep(Duration::from_millis(10)),
            }
        };
        let mut out = String::new();
        child
            .stdout
            .take()?
            .take(4096)
            .read_to_string(&mut out)
            .ok()?;
        status.success().then(|| repository_key_of(&out)).flatten()
    }
}
