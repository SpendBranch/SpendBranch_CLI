//! `login`, `logout`, `status`, `enable` and `disable`: the device's account and its sources.
use super::{Ctx, Result, say};
use crate::api::{
    ApiFailure, CliStatus, DEFAULT_SERVER, PairingStarted, PollOutcome, PollToken, Polled,
    ServerUrl, SourceState, StartPairing, StatusView,
};
use crate::auth::{DeviceId, DeviceKey};
use crate::cli::{VERSION, messages as m, render};
use crate::contract::{Iso, Source};
use crate::infra::{
    clock::{Clock, SystemClock},
    http::{Client, Signer},
    keys::{self, Backend, KeyStore, Slot},
    lock,
    store::Store,
};
use crate::readers::{claude_code, codex};
use crate::sources::{claude_code as claude_parser, codex as codex_parser};
use std::{path::PathBuf, time::Duration};

const LOGOUT: &str = "/api/cli/v1/logout";
const STATUS: &str = "/api/cli/v1/status";
/// The longest the polling interval grows to after repeated `slow_down` answers.
const MAX_INTERVAL_SECS: u64 = 120;
/// A passing network or server trouble while polling is tried again this many times in a row.
const POLL_RETRIES: u32 = 3;
const LOCK_TRIES: u32 = 60;

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

fn failure(ctx: &mut Ctx, f: &ApiFailure) -> Result<i32> {
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

fn fetch_status(c: &Connection) -> std::result::Result<StatusView, String> {
    status_of(c).map_err(|f| {
        if f.code == "BAD_RESPONSE" {
            m::BAD_STATUS.to_owned()
        } else {
            m::failure_text(&f, SystemClock.now_ms())
        }
    })
}

fn revoke(c: &Connection) -> std::result::Result<(), ApiFailure> {
    let signer = Signer {
        device: &c.device,
        key: &c.key,
    };
    Client::new(c.server.clone(), VERSION).post_empty(LOGOUT, Some(&signer))
}

// --- login ----------------------------------------------------------------------------

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

// --- logout ---------------------------------------------------------------------------

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
    say(ctx, m::LOGOUT_LOCAL_DELETED, 0)
}

// --- status ---------------------------------------------------------------------------

fn folder_of(source: Source, ctx: &Ctx) -> String {
    let folder = match source {
        Source::ClaudeCode => claude_code::Config::from_env(ctx.env).map(|c| c.projects_dir),
        Source::Codex => codex::Config::from_env(ctx.env).map(|c| c.root),
    };
    folder.map_or_else(|| "unknown".into(), |p| p.display().to_string())
}

fn local_status(ctx: &Ctx, store: &Store) -> Result<render::Local> {
    let (items, coverage) = store.pending_counts()?;
    let mut sources = Vec::new();
    for source in [Source::ClaudeCode, Source::Codex] {
        sources.push(render::LocalSource {
            source,
            facts: store.source_facts(source)?,
            folder: folder_of(source, ctx),
        });
    }
    Ok(render::Local {
        items,
        coverage,
        rejected: store.rejected_counts()?,
        last_sent: store.sync_get("lastSentAt")?,
        last_error: store.sync_get("lastError")?,
        next_attempt: store.sync_get("nextAttemptAt")?,
        sources,
    })
}

fn print_lines(ctx: &mut Ctx, lines: Vec<String>) -> Result<()> {
    for line in lines {
        writeln!(ctx.out, "{line}")?;
    }
    Ok(())
}

pub fn status(ctx: &mut Ctx) -> Result<i32> {
    let account = open(ctx)?;
    let local = local_status(ctx, &account.store)?;
    let connection = match load(&account)? {
        Loaded::NotConnected => return say(ctx, m::NOT_CONNECTED, 1),
        Loaded::Broken(text) => {
            say(ctx, &text, 1)?;
            print_lines(ctx, render::local_lines(&local, now()))?;
            return Ok(1);
        }
        Loaded::Ready(c) => c,
    };
    match fetch_status(&connection) {
        Ok(view) => {
            let file = account.keys.backend() == Backend::File;
            let lines = render::remote_lines(
                &view,
                &connection.server.base(),
                &connection.key.fingerprint(),
                file,
                &local,
                now(),
            );
            print_lines(ctx, lines)?;
            print_lines(ctx, render::local_lines(&local, now()))?;
            Ok(0)
        }
        Err(text) => {
            say(ctx, &text, 1)?;
            print_lines(ctx, render::local_lines(&local, now()))?;
            Ok(1)
        }
    }
}

// --- enable / disable -----------------------------------------------------------------

fn parser_of(source: Source) -> (&'static str, &'static [&'static str]) {
    match source {
        Source::ClaudeCode => (claude_parser::PARSER, &claude_parser::FIELDS),
        Source::Codex => (codex_parser::PARSER, &codex_parser::FIELDS),
    }
}

pub fn enable(ctx: &mut Ctx, source: Source) -> Result<i32> {
    let account = open(ctx)?;
    let connection = match load(&account)? {
        Loaded::NotConnected => return say(ctx, m::NOT_CONNECTED, 1),
        Loaded::Broken(text) => return say(ctx, &text, 1),
        Loaded::Ready(c) => c,
    };
    let view = match fetch_status(&connection) {
        Ok(v) => v,
        Err(text) => return say(ctx, &text, 1),
    };
    let agent = render::agent_name(source);
    let Some(granted) = view
        .sources
        .iter()
        .find(|s| s.source == source && s.state == SourceState::Active)
    else {
        return say(ctx, &m::not_authorized(agent), 1);
    };
    // Every field this reader reads must be on the owner's list, the same names in the same
    // order: anything else is refused at upload (`FIELDS_NOT_AUTHORIZED`).
    let (parser, fields) = parser_of(source);
    if granted.parser_version.as_str() != parser
        || !granted
            .fields
            .iter()
            .map(String::as_str)
            .eq(fields.iter().copied())
    {
        return say(ctx, &m::different_fields(agent), 1);
    }
    writeln!(ctx.out, "Fields read from {agent} sessions:")?;
    for field in fields {
        writeln!(ctx.out, "  {field}")?;
    }
    // Already on: keep the date it started, or the first days would fall out of the window.
    if account.store.source_facts(source)?.enabled_since.is_none() {
        account.store.enable_source(source, now())?;
    }
    writeln!(
        ctx.out,
        "Collection on for {agent}. Reading: {}",
        folder_of(source, ctx)
    )?;
    Ok(0)
}

pub fn disable(ctx: &mut Ctx, source: Source) -> Result<i32> {
    let account = open(ctx)?;
    if !account.store.is_connected()? {
        return say(ctx, m::NOT_CONNECTED, 1);
    }
    account.store.disable_source(source)?;
    say(ctx, m::DISABLED, 0)
}
