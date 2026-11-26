//! `logout`: revoke the device on the server and delete what is local.
use super::super::{Ctx, Result, say};
use super::{Loaded, load, open, revoke};
use crate::cli::messages as m;
use crate::infra::{
    keys::{self, Slot},
    scheduler,
};

/// What the server said about the revocation.
pub enum Revoked {
    Yes,
    /// Revoked, expired or without access already.
    Already,
    /// Not reached, or it refused: the device must be revoked on the web.
    No,
    /// Not asked: the stored key must not be used (the text says why). Revoke it on the web.
    NotTried(String),
}

/// What `disconnect` did.
pub enum Disconnected {
    NotConnected,
    /// The local key and the device's ids are gone; `kept` says why a stored key could not be
    /// deleted, when that happened.
    Done {
        revoked: Revoked,
        kept: Vec<String>,
    },
}

/// The request to the server, ready to run.
pub type Request<'a> = &'a (dyn Fn() -> Revoked + Sync);
/// Runs the request: at once (`logout`), or behind a spinner (`uninstall`).
pub type Wait<'a> = &'a mut dyn FnMut(&mut Ctx, Request) -> Result<Revoked>;

/// Revokes the device on the server (through `wait`) and deletes the local key and ids.
/// Says nothing.
pub fn disconnect(ctx: &mut Ctx, wait: Wait) -> Result<Disconnected> {
    let account = open(ctx)?;
    if !account.store.is_connected()? {
        return Ok(Disconnected::NotConnected);
    }
    let revoked = match load(&account)? {
        Loaded::Ready(c) => wait(ctx, &|| match revoke(&c) {
            Ok(()) => Revoked::Yes,
            Err(f) if f.code == "UNAUTHENTICATED" => Revoked::Already,
            Err(_) => Revoked::No,
        })?,
        _ => match account.keys.load(Slot::Active) {
            Err(e @ keys::Error::KeyOutsideProfile(_)) => Revoked::NotTried(e.to_string()),
            _ => Revoked::No,
        },
    };
    // Whatever the server said, this device stops being able to sign. The organization's id
    // and the server stay, so a later login to another organization is noticed.
    let mut kept = Vec::new();
    for slot in [Slot::Active, Slot::Pending] {
        if let Err(e) = account.keys.delete(slot) {
            kept.push(e.to_string());
        }
    }
    for key in ["device_id", "org_name", "expires_at"] {
        account.store.meta_delete(key)?;
    }
    account.store.forget_grants()?;
    Ok(Disconnected::Done { revoked, kept })
}

pub fn logout(ctx: &mut Ctx) -> Result<i32> {
    let Disconnected::Done { revoked, kept } = disconnect(ctx, &mut |_, request| Ok(request()))?
    else {
        return say(ctx, m::ALREADY_DISCONNECTED, 0);
    };
    let said = match revoked {
        Revoked::Yes => m::LOGOUT_REVOKED,
        Revoked::Already => m::LOGOUT_ALREADY_REVOKED,
        Revoked::No => m::LOGOUT_NOT_REVOKED,
        Revoked::NotTried(why) => {
            say(ctx, &why, 0)?;
            m::LOGOUT_NOT_REVOKED
        }
    };
    say(ctx, said, 0)?;
    for why in kept {
        writeln!(ctx.out, "Could not delete the stored key: {why}")?;
    }
    say(ctx, m::LOGOUT_LOCAL_DELETED, 0)?;
    match scheduler::remove(ctx.system, ctx.env, ctx.system.platform()) {
        Ok(true) => say(ctx, m::SCHEDULER_REMOVED, 0),
        Ok(false) => Ok(0),
        Err(why) => say(ctx, &m::scheduler_not_removed(&why), 0),
    }
}
