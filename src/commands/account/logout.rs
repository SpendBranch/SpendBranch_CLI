//! `logout`: revoke the device on the server and delete what is local.
use super::super::{Ctx, Result, say};
use super::{Loaded, load, open, revoke};
use crate::cli::messages as m;
use crate::infra::{keys::Slot, scheduler};

pub fn logout(ctx: &mut Ctx) -> Result<i32> {
    let account = open(ctx)?;
    if !account.store.is_connected()? {
        return say(ctx, crate::cli::messages::ALREADY_DISCONNECTED, 0);
    }
    let said = match load(&account)? {
        Loaded::Ready(c) => match revoke(&c) {
            Ok(()) => m::LOGOUT_REVOKED,
            Err(f) if f.code == "UNAUTHENTICATED" => m::LOGOUT_ALREADY_REVOKED,
            Err(_) => m::LOGOUT_NOT_REVOKED,
        },
        _ => m::LOGOUT_NOT_REVOKED,
    };
    say(ctx, said, 0)?;
    // Whatever the server said, this device stops being able to sign. The organization's id
    // and the server stay, so a later login to another organization is noticed.
    for slot in [Slot::Active, Slot::Pending] {
        if let Err(e) = account.keys.delete(slot) {
            writeln!(ctx.out, "Could not delete the stored key: {e}")?;
        }
    }
    for key in ["device_id", "org_name", "expires_at"] {
        account.store.meta_delete(key)?;
    }
    say(ctx, m::LOGOUT_LOCAL_DELETED, 0)?;
    match scheduler::remove(ctx.system, ctx.env, ctx.system.platform()) {
        Ok(true) => say(ctx, m::SCHEDULER_REMOVED, 0),
        Ok(false) => Ok(0),
        Err(why) => say(ctx, &m::scheduler_not_removed(&why), 0),
    }
}
