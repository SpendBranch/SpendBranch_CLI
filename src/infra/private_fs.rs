//! Owner-only files and folders: 0700 / 0600 on Unix. On Windows the protection
//! is the ACL of the user's own profile folder, so nothing extra is done there.
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

/// Makes sure the folder exists. A folder we create is 0700. A folder that already exists
/// is never touched (it may be the user's home or a shared folder chosen by mistake), but it
/// is refused if anybody could write in it: another user could plant the files we open.
/// Group write is allowed: with a private group per user, the Debian default, it is normal.
pub fn create_dir(path: &Path) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(meta) => check_existing(path, &meta),
        Err(e) if e.kind() == io::ErrorKind::NotFound => create_new(path),
        Err(e) => Err(e),
    }
}

fn create_new(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path)
    }
}

fn check_existing(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    if !meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} exists and is not a folder", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o002 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} can be written by any user; make it private (chmod 700) or pick another folder with SPENDBRANCH_HOME",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Opens (creating if needed) a file that only the owner can read or write.
pub fn open(path: &Path, append: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true);
    if append {
        options.append(true);
    } else {
        options.write(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// Replaces the content of an owner-only file.
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    let mut file = open(path, false)?;
    file.set_len(0)?;
    file.write_all(text.as_bytes())
}
