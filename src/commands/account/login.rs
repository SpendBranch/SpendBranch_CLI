//! `login`: pair this device with the person's organization.
use super::super::{Ctx, Result, say};
use super::status::local_status;
use super::{Account, Connection, failure, fetch_status, now, open, revoke};
use crate::api::{
    ApiFailure, DEFAULT_SERVER, PairingStarted, PollOutcome, PollToken, Polled, ServerUrl,
    StartPairing,
};
use crate::auth::{DeviceId, DeviceKey};
use crate::cli::{VERSION, messages as m, render};
use crate::infra::{
    http::Client,
    keys::{self, Backend, Slot},
    lock,
};
use std::time::Duration;
/// The longest the polling interval grows to after repeated `slow_down` answers.
const MAX_INTERVAL_SECS: u64 = 120;
/// A passing network or server trouble while polling is tried again this many times in a row.
const POLL_RETRIES: u32 = 3;
const LOCK_TRIES: u32 = 60;

enum Paired {
    Approved(crate::api::Approval),
    /// Said why on screen; this is the exit code.
    Stopped(i32),
}

pub fn login(ctx: &mut Ctx, server: Option<String>) -> Result<i32> {
    let text = server
        .or_else(|| (ctx.env)("SPENDBRANCH_SERVER").filter(|s| !s.is_empty()))
        .unwrap_or_else(|| DEFAULT_SERVER.into());
    let server = match ServerUrl::parse(&text) {
        Ok(s) => s,
        Err(e) => return say(ctx, &format!("{}: {e}", m::INVALID_SERVER), 2),
    };
    let mut account = open(ctx)?;
    // The new key waits in its own slot: a login that fails leaves the working key alone.
    let key = keys::generate()?;
    account.keys.save(Slot::Pending, &key)?;
    let result = connect(ctx, &mut account, &server, &key);
    let _ = account.keys.delete(Slot::Pending);
    result
}

fn connect(
    ctx: &mut Ctx,
    account: &mut Account,
    server: &ServerUrl,
    key: &DeviceKey,
) -> Result<i32> {
    let installation_id = account.store.installation_id()?;
    let approval = match pair(ctx, server, key, &installation_id)? {
        Paired::Approved(a) => a,
        Paired::Stopped(code) => return Ok(code),
    };
    let new = Connection {
        server: server.clone(),
        device: approval.device_id.clone(),
        key: DeviceKey::from_text(&key.to_text()).ok_or("the new key cannot be read back")?,
    };
    writeln!(ctx.out, "Organization: {}", approval.organization_name)?;
    write!(
        ctx.out,
        "{}",
        m::confirm_organization(&approval.organization_name)
    )?;
    ctx.out.flush()?;
    let mut answer = String::new();
    ctx.input.read_line(&mut answer)?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        let _ = revoke(&new); // the server already made the device: take it back
        return say(ctx, m::LOGIN_CANCELLED, 1);
    }
    // Saving changes the database the `sync` works on: wait for it to finish.
    let mut held = None;
    for attempt in 0..LOCK_TRIES {
        held = lock::try_acquire(&account.dir)?;
        if held.is_some() {
            break;
        }
        if attempt + 1 < LOCK_TRIES {
            (ctx.sleep)(Duration::from_secs(1));
        }
    }
    let Some(_lock) = held else {
        let _ = revoke(&new);
        return say(ctx, m::LOGIN_LOCK_BUSY, 1);
    };

    let (old_device, old_org, old_server) = (
        account.store.meta_get("device_id")?,
        account.store.meta_get("org_id")?,
        account.store.meta_get("server")?,
    );
    if let Some(old) = old_device.filter(|d| *d != approval.device_id.as_str()) {
        let previous = old_server
            .as_deref()
            .and_then(|s| ServerUrl::parse(s).ok())
            .zip(DeviceId::new(&old))
            .zip(account.keys.load(Slot::Active).ok().flatten())
            .map(|((server, device), key)| Connection {
                server,
                device,
                key,
            });
        // "Already revoked" is fine: the server replaces the old device when the new one is approved.
        let gone = previous.is_some_and(|p| match revoke(&p) {
            Ok(()) => true,
            Err(f) => f.code == "UNAUTHENTICATED",
        });
        if !gone {
            say(ctx, m::OLD_DEVICE_NOT_REVOKED, 0)?;
        }
    }
    // Compared by the organization's id, never its name.
    let other_org = old_org.is_some_and(|o| o != approval.organization_id)
        || old_server.is_some_and(|s| s != server.base());
    if other_org {
        account.store.discard_buffer()?;
        account.store.disable_all_sources()?;
        say(ctx, m::BUFFER_DISCARDED, 0)?;
    }
    account.keys.save(Slot::Active, key)?;
    let store = &account.store;
    store.meta_set("device_id", approval.device_id.as_str())?;
    store.meta_set("server", &server.base())?;
    store.meta_set("org_id", &approval.organization_id)?;
    store.meta_set("org_name", &approval.organization_name)?;
    store.meta_set("expires_at", &approval.expires_at.to_string())?;
    if account.keys.backend() == Backend::File {
        say(ctx, m::KEY_IN_FILE, 0)?;
    }
    say(ctx, &m::ready(&approval.organization_name), 0)?;
    // What the company authorized, as `status` shows it. A failure here does not undo the login.
    if let Ok(view) = fetch_status(&new) {
        let local = local_status(ctx, &account.store)?;
        for line in render::remote_lines(
            &view,
            &server.base(),
            &new.key.fingerprint(),
            account.keys.backend() == Backend::File,
            &local,
            now(),
        ) {
            writeln!(ctx.out, "{line}")?;
        }
    }
    Ok(0)
}

fn pair(
    ctx: &mut Ctx,
    server: &ServerUrl,
    key: &DeviceKey,
    installation_id: &str,
) -> Result<Paired> {
    let client = Client::new(server.clone(), VERSION);
    let start = StartPairing {
        installation_id,
        cli_version: Some(VERSION),
        public_key: &key.public_key(),
    };
    let started: PairingStarted = match client.post("/api/cli/v1/device-codes", &start, None) {
        Ok(s) => s,
        Err(f) => return Ok(Paired::Stopped(failure(ctx, &f)?)),
    };
    let Some(started) = started.checked(server) else {
        return Ok(Paired::Stopped(say(ctx, m::LOGIN_BAD_PAIRING, 1)?));
    };
    writeln!(
        ctx.out,
        "Open this address in your browser to approve this device:\n  {}\nCode: {}\nIt expires in {} minutes.",
        started.verification_uri_complete,
        started.user_code,
        started.expires_in / 60
    )?;
    if ctx.interactive && (ctx.env)("SPENDBRANCH_NO_BROWSER").is_none() {
        (ctx.open_url)(&started.verification_uri_complete);
    }
    let (mut interval, mut waited, mut trouble) = (started.interval, 0u64, 0u32);
    loop {
        // Do not wait for a poll that would come after the code is dead.
        if waited + interval > started.expires_in {
            return Ok(Paired::Stopped(say(ctx, m::LOGIN_EXPIRED, 1)?));
        }
        (ctx.sleep)(Duration::from_secs(interval));
        waited += interval;
        let poll = PollToken {
            device_code: &started.device_code,
        };
        match client.post::<_, PollOutcome>("/api/cli/v1/tokens", &poll, None) {
            Ok(outcome) => {
                trouble = 0;
                match outcome.checked() {
                    None => {
                        return Ok(Paired::Stopped(failure(
                            ctx,
                            &ApiFailure::bad_response(200),
                        )?));
                    }
                    Some(Polled::Pending) => {}
                    Some(Polled::SlowDown) => interval = (interval + 5).min(MAX_INTERVAL_SECS),
                    Some(Polled::Expired) => {
                        return Ok(Paired::Stopped(say(ctx, m::LOGIN_EXPIRED, 1)?));
                    }
                    Some(Polled::Denied) => {
                        return Ok(Paired::Stopped(say(ctx, m::LOGIN_DENIED, 1)?));
                    }
                    Some(Polled::Approved(approval)) => return Ok(Paired::Approved(approval)),
                }
            }
            // The network or the server stumbled: that is not a reason to throw the pairing away.
            Err(f) if (f.status == 0 || f.status >= 500) && trouble < POLL_RETRIES => trouble += 1,
            Err(f) if f.code == "RATE_LIMITED" => {
                interval = interval.max(f.retry_after_secs.unwrap_or(0).min(MAX_INTERVAL_SECS));
            }
            Err(f) => return Ok(Paired::Stopped(failure(ctx, &f)?)),
        }
    }
}
