//! `login`, `logout`, `status`, `enable` and `disable`: the device's account and its sources.
use super::{Ctx, Result, say};
use crate::api::{ApiFailure, CliStatus, ServerUrl, StatusView};
use crate::auth::{DeviceId, DeviceKey};
use crate::cli::{VERSION, messages as m};
use crate::contract::Iso;
use crate::infra::{
    clock::{Clock, SystemClock},
    http::{Client, Signer},
    keys::{self, KeyStore, Slot},
    store::Store,
};
use std::path::PathBuf;

const LOGOUT: &str = "/api/cli/v1/logout";
const STATUS: &str = "/api/cli/v1/status";

pub(super) fn now() -> Iso {
    Iso::from_millis(SystemClock.now_ms()).unwrap_or(Iso::EPOCH)
}

/// The data folder, the database and the key store, opened.
pub(super) struct Account {
    pub(super) dir: PathBuf,
    pub(super) store: Store,
    pub(super) keys: Box<dyn KeyStore>,
}

pub(super) fn open(ctx: &Ctx) -> Result<Account> {
    let dir = crate::infra::paths::data_dir(ctx.env)?;
    let store = Store::open(&dir)?;
    // `SPENDBRANCH_KEY_STORE=file` keeps the key in a file even where a keychain exists:
    // for a headless box, and for tests that must never touch the real keychain.
    let keys: Box<dyn KeyStore> = if (ctx.env)("SPENDBRANCH_KEY_STORE").as_deref() == Some("file") {
        Box::new(keys::KeyFiles::new(&dir))
    } else {
        keys::open(&dir, &store.installation_id()?)
    };
    Ok(Account { dir, store, keys })
}

/// A paired device: where it lives, who it is and the key that signs for it.
pub(super) struct Connection {
    pub(super) server: ServerUrl,
    pub(super) device: DeviceId,
    pub(super) key: DeviceKey,
}

pub(super) enum Loaded {
    Ready(Box<Connection>),
    NotConnected,
    /// Paired, but what was saved cannot be used: the text says what to do.
    Broken(String),
}

pub(super) fn load(account: &Account) -> Result<Loaded> {
    let Some(device) = account.store.meta_get("device_id")? else {
        return Ok(Loaded::NotConnected);
    };
    let broken =
        || Loaded::Broken("The saved connection is not valid. Run `spendbranch login`.".into());
    let (Some(server), Some(device)) = (
        account
            .store
            .meta_get("server")?
            .and_then(|s| ServerUrl::parse(&s).ok()),
        DeviceId::new(&device),
    ) else {
        return Ok(broken());
    };
    Ok(match account.keys.load(Slot::Active) {
        Ok(Some(key)) => Loaded::Ready(Box::new(Connection {
            server,
            device,
            key,
        })),
        Ok(None) | Err(keys::Error::Corrupt) => Loaded::Broken(m::KEY_MISSING.into()),
        Err(e) => Loaded::Broken(e.to_string()),
    })
}

pub(super) fn failure(ctx: &mut Ctx, f: &ApiFailure) -> Result<i32> {
    say(ctx, &m::failure_text(f, SystemClock.now_ms()), 1)
}

pub(super) fn status_of(c: &Connection) -> std::result::Result<StatusView, ApiFailure> {
    let signer = Signer {
        device: &c.device,
        key: &c.key,
    };
    Client::new(c.server.clone(), VERSION)
        .get::<CliStatus>(STATUS, Some(&signer))?
        .checked()
        .ok_or_else(|| ApiFailure::bad_response(200))
}

pub(super) fn fetch_status(c: &Connection) -> std::result::Result<StatusView, String> {
    status_of(c).map_err(|f| {
        if f.code == "BAD_RESPONSE" {
            m::BAD_STATUS.to_owned()
        } else {
            m::failure_text(&f, SystemClock.now_ms())
        }
    })
}

pub(super) fn revoke(c: &Connection) -> std::result::Result<(), ApiFailure> {
    let signer = Signer {
        device: &c.device,
        key: &c.key,
    };
    Client::new(c.server.clone(), VERSION).post_empty(LOGOUT, Some(&signer))
}

mod login;
mod logout;
mod sources;
mod status;

pub use login::login;
pub use logout::logout;
pub(in crate::commands) use sources::enable_with;
pub use sources::{disable, enable};
pub use status::status;
