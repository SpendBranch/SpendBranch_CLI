//! Where the device keeps its data.
use crate::env::Env;
use std::{io, path::PathBuf};

/// `$SPENDBRANCH_HOME` > `$XDG_CONFIG_HOME/spendbranch` > `%APPDATA%\spendbranch` (Windows)
/// or `~/.config/spendbranch`. Empty variables count as unset. `SPENDBRANCH_HOME` must be absolute
/// (an error otherwise); a relative `XDG_CONFIG_HOME` is ignored.
pub fn data_dir(env: Env) -> io::Result<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(home) = get("SPENDBRANCH_HOME") {
        // Hooks and the scheduler start us from any folder: a relative path would split the
        // state in as many databases as there are folders. Fail loudly, do not guess.
        let home = PathBuf::from(home);
        if !home.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SPENDBRANCH_HOME must be an absolute path",
            ));
        }
        return Ok(home);
    }
    // The XDG rule: a relative XDG_CONFIG_HOME is invalid and must be ignored.
    if let Some(xdg) = get("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Ok(xdg.join("spendbranch"));
    }
    let (base, var) = if cfg!(windows) {
        (PathBuf::new(), "APPDATA")
    } else {
        (PathBuf::from(".config"), "HOME")
    };
    let root = get(var).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("cannot locate the data folder: {var} is not set"),
        )
    })?;
    Ok(PathBuf::from(root).join(base).join("spendbranch"))
}
