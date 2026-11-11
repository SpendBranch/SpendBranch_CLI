//! Owner-only files and folders: 0700 / 0600 on Unix. On Windows the protection
//! is the ACL of the user's own profile folder, so nothing extra is done there.
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

/// Creates the folder (and parents) and forces 0700 on the leaf.
pub fn create_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
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
