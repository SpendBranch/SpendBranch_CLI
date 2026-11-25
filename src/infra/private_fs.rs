//! Owner-only files and folders: 0700 / 0600 on Unix. On Windows the protection
//! is the ACL of the user's own profile folder, so nothing extra is done there (the key file
//! store refuses a folder outside the profile: see `keys::KeyFiles::in_profile`).
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

/// Makes sure the folder exists. A folder we create is 0700. A folder that already exists
/// is never touched (it may be the user's home or a shared folder chosen by mistake), but it
/// is refused unless only this user can write in it (`folder_is_private`): anyone else could
/// plant the files we open, or swap the environment file the user loads.
pub fn create_dir(path: &Path) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(meta) => check_existing(path, &meta),
        Err(e) if e.kind() == io::ErrorKind::NotFound => create_new(path),
        Err(e) => Err(e),
    }
}

/// Only this user can write in a folder of this `mode`, `owner` and `group` (`me` is this
/// process's user): it is ours, nobody else may write, and group write only with the user's
/// own private group. That group is recognised by its number being the user's (the Debian,
/// Ubuntu and Fedora convention).
// ponytail: the private group is guessed from gid == uid; reading /etc/group would know for sure.
pub fn folder_is_private(mode: u32, owner: u32, group: u32, me: u32) -> bool {
    owner == me && mode & 0o002 == 0 && (mode & 0o020 == 0 || group == owner)
}

/// This process's effective user, without `unsafe`: the owner of `/proc/self` on Linux, else
/// of a file just made in the user's temporary folder. `/proc/self` of a process that is not
/// dumpable belongs to root, so root from there is checked with the file. Worked out once.
#[cfg(unix)]
pub(crate) fn my_uid() -> io::Result<u32> {
    use std::os::unix::fs::MetadataExt;
    static UID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    if let Some(uid) = UID.get() {
        return Ok(*uid);
    }
    let from_proc = fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let uid = match from_proc {
        Some(uid) if uid != 0 => uid,
        _ => {
            // The counter keeps two threads of one process apart: macOS clocks tick in
            // microseconds, so the time alone repeats.
            static PROBE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = PROBE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let probe = std::env::temp_dir()
                .join(format!(".spendbranch-{}-{n}-{nanos}", std::process::id()));
            let uid = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&probe)?
                .metadata()?
                .uid();
            let _ = fs::remove_file(&probe);
            uid
        }
    };
    Ok(*UID.get_or_init(|| uid))
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
        use std::os::unix::fs::MetadataExt;
        if !folder_is_private(meta.mode(), meta.uid(), meta.gid(), my_uid()?) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} can be written by other users or belongs to someone else; make it yours and private (chmod 700) or pick another folder with SPENDBRANCH_HOME",
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
        // A symlink planted under one of our names is never followed: no file elsewhere is
        // truncated, appended to or chmodded through it.
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
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
