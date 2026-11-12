//! Where the device's private key lives: the system keychain, or a `0600` file when there
//! is no keychain (a headless Linux box). The key is never logged or put in an error.
use super::private_fs;
use crate::auth::DeviceKey;
use keyring_core::{CredentialStore, Entry};
use std::{
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

const SERVICE: &str = "spendbranch-cli";

/// Two keys can exist at once while a device is re-paired: the old one keeps working until
/// the new one is approved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Active,
    Pending,
}

impl Slot {
    fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Pending => "pending",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Keychain,
    /// Less safe: the key sits in the data folder. `login` and `status` say so.
    File,
}

#[derive(Debug)]
pub enum Error {
    /// The keychain exists but refused (locked, no session, denied).
    Unavailable,
    /// What is stored is not a key.
    Corrupt,
    /// The key file can be read by other users; the key is treated as exposed.
    Insecure(PathBuf),
    /// The system has no source of randomness.
    Entropy,
    Io(io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("the system keychain is not available"),
            Self::Corrupt => {
                f.write_str("the stored device key is not valid; run `spendbranch login`")
            }
            Self::Insecure(p) => write!(
                f,
                "{} can be read by other users; delete it and run `spendbranch login`",
                p.display()
            ),
            Self::Entropy => f.write_str("the system has no source of randomness"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub trait KeyStore {
    fn backend(&self) -> Backend;
    fn load(&self, slot: Slot) -> Result<Option<DeviceKey>>;
    fn save(&self, slot: Slot, key: &DeviceKey) -> Result<()>;
    /// Not an error if there is nothing to delete.
    fn delete(&self, slot: Slot) -> Result<()>;
}

/// A new random key.
pub fn generate() -> Result<DeviceKey> {
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut seed[..]).map_err(|_| Error::Entropy)?;
    Ok(DeviceKey::from_seed(&seed))
}

/// The keychain if it works and no key file exists yet, else the file. A key file that is
/// already there wins, so a key saved without a keychain is not lost when one appears.
pub fn open(dir: &Path, installation_id: &str) -> Box<dyn KeyStore> {
    let files = KeyFiles::new(dir);
    if !files.exists()
        && let Some(keychain) = Keychain::platform(installation_id)
    {
        return Box::new(keychain);
    }
    Box::new(files)
}

// --- System keychain ------------------------------------------------------------------

pub struct Keychain {
    store: Arc<CredentialStore>,
    installation_id: String,
}

impl Keychain {
    /// The platform's keychain, or `None` when it cannot be used right now.
    pub fn platform(installation_id: &str) -> Option<Self> {
        let keychain = Self::with_store(platform_store()?, installation_id);
        // Reading is how we learn that the keychain is usable: "no entry" is a yes.
        keychain.load(Slot::Active).is_ok().then_some(keychain)
    }

    pub fn with_store(store: Arc<CredentialStore>, installation_id: &str) -> Self {
        Self {
            store,
            installation_id: installation_id.to_owned(),
        }
    }

    fn entry(&self, slot: Slot) -> Result<Entry> {
        let user = format!("{}/{}", self.installation_id, slot.name());
        self.store
            .build(SERVICE, &user, None)
            .map_err(|_| Error::Unavailable)
    }
}

#[cfg(target_os = "macos")]
fn platform_store() -> Option<Arc<CredentialStore>> {
    Some(apple_native_keyring_store::keychain::Store::new().ok()?)
}

#[cfg(target_os = "windows")]
fn platform_store() -> Option<Arc<CredentialStore>> {
    Some(windows_native_keyring_store::Store::new().ok()?)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_store() -> Option<Arc<CredentialStore>> {
    Some(zbus_secret_service_keyring_store::Store::new().ok()?)
}

impl KeyStore for Keychain {
    fn backend(&self) -> Backend {
        Backend::Keychain
    }

    fn load(&self, slot: Slot) -> Result<Option<DeviceKey>> {
        match self.entry(slot)?.get_password() {
            Ok(text) => DeviceKey::from_text(&Zeroizing::new(text))
                .map(Some)
                .ok_or(Error::Corrupt),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(_) => Err(Error::Unavailable),
        }
    }

    fn save(&self, slot: Slot, key: &DeviceKey) -> Result<()> {
        self.entry(slot)?
            .set_password(&key.to_text())
            .map_err(|_| Error::Unavailable)
    }

    fn delete(&self, slot: Slot) -> Result<()> {
        match self.entry(slot)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(_) => Err(Error::Unavailable),
        }
    }
}

// --- File -----------------------------------------------------------------------------

pub struct KeyFiles {
    dir: PathBuf,
}

impl KeyFiles {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
        }
    }

    fn path(&self, slot: Slot) -> PathBuf {
        self.dir.join(match slot {
            Slot::Active => "device.key",
            Slot::Pending => "device.key.pending",
        })
    }

    fn exists(&self) -> bool {
        [Slot::Active, Slot::Pending]
            .iter()
            .any(|s| self.path(*s).exists())
    }
}

impl KeyStore for KeyFiles {
    fn backend(&self) -> Backend {
        Backend::File
    }

    fn load(&self, slot: Slot) -> Result<Option<DeviceKey>> {
        let path = self.path(slot);
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if file.metadata()?.permissions().mode() & 0o077 != 0 {
                return Err(Error::Insecure(path));
            }
        }
        let mut text = Zeroizing::new(String::new());
        // A key is 43 characters: read a little more than that and no further.
        file.take(256).read_to_string(&mut text)?;
        DeviceKey::from_text(&text).map(Some).ok_or(Error::Corrupt)
    }

    fn save(&self, slot: Slot, key: &DeviceKey) -> Result<()> {
        private_fs::create_dir(&self.dir)?;
        // Write beside it, then rename: a crash leaves the old key or the new one, never half.
        // The name has the slot and the process, so two writers never share a temporary file.
        let temp = self.dir.join(format!(
            "device.key.{}.{}.tmp",
            slot.name(),
            std::process::id()
        ));
        let written = (|| {
            let mut file = private_fs::open(&temp, false)?;
            file.set_len(0)?;
            file.write_all(key.to_text().as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, self.path(slot))
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temp); // never leave a copy of the key lying around
        }
        Ok(written?)
    }

    fn delete(&self, slot: Slot) -> Result<()> {
        match fs::remove_file(self.path(slot)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
