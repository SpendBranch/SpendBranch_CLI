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
        path.is_absolute()
            && path.parent().is_some()
            && self.home.as_deref() != Some(path)
            && !is_network_path(cwd)
    }

    /// The command that asks git, without running it.
    pub fn build_command(&self, cwd: &str) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(["-C", cwd, "remote", "get-url", "origin"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // `-C` picks the repository; variables inherited from the user's shell must not
        // redirect git somewhere else.
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_COMMON_DIR",
            "GIT_NAMESPACE",
        ] {
            command.env_remove(var);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        command
    }

    fn ask_git(&self, cwd: &str) -> Option<String> {
        let mut command = self.build_command(cwd);
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

/// A network share (`\\host\share`, `//host/share`, `\\?\UNC\...`). The folder comes from a
/// transcript, so it must never make this machine contact another one.
pub fn is_network_path(path: &str) -> bool {
    let p = path.replace('/', "\\");
    if p.starts_with("\\\\?\\UNC\\") || p.starts_with("\\\\.\\UNC\\") {
        return true;
    }
    p.starts_with("\\\\") && !p.starts_with("\\\\?\\") && !p.starts_with("\\\\.\\")
}
