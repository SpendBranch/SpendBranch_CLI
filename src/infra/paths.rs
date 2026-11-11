//! Where the device keeps its data.
use std::{io, path::PathBuf};

/// `$SPENDBRANCH_HOME` > `$XDG_CONFIG_HOME/spendbranch` > `%APPDATA%\spendbranch` (Windows)
/// or `~/.config/spendbranch`. Empty variables count as unset.
pub fn data_dir(env: &dyn Fn(&str) -> Option<String>) -> io::Result<PathBuf> {
    let get = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(home) = get("SPENDBRANCH_HOME") {
        return Ok(PathBuf::from(home));
    }
    if let Some(xdg) = get("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("spendbranch"));
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
