//! `sync`: read what is new from the sources that are on, then send the buffer in batches.
//! Nothing leaves the buffer until the server answered for it, so a crash loses nothing.
use super::account::{self, Connection, Loaded, now, open, status_of};
use super::{Ctx, Result, say};
use crate::api::{ApiFailure, IngestResults, SourceState};
use crate::cli::{VERSION, messages as m, render};
use crate::contract::{Iso, Source};
use crate::infra::{
    clock::{Clock, SystemClock},
    http::{Client, Signer},
    lock,
    store::{Change, Commit, Queued, SpoolLimits, Store},
};
use crate::readers::{claude_code, codex};
use crate::sync::{self as rules, Failed, TRIES_PER_RUN};
use std::collections::BTreeMap;

const INGEST: &str = "/api/ingest/v1/events";
const MAX_PASSES: usize = 100_000;
const BOTH: [Source; 2] = [Source::ClaudeCode, Source::Codex];

/// What this run did, for the last lines.
#[derive(Default)]
struct Tally {
    collected: u64,
    accepted: u64,
    sent_any: bool,
    rejected: BTreeMap<String, u64>,
    /// Observations of a source the company has not (or no longer) authorized: kept, not sent.
    held: BTreeMap<&'static str, u64>,
}

impl Tally {
    fn add(&mut self, code: &str, n: u64) {
        *self.rejected.entry(code.to_owned()).or_default() += n;
    }
}

fn random() -> f64 {
    let mut bytes = [0u8; 8];
    // Without entropy the jitter is just the middle of the range: a wait is still a wait.
    if getrandom::fill(&mut bytes).is_err() {
        return 0.5;
    }
    u64::from_le_bytes(bytes) as f64 / u64::MAX as f64
}

fn ms() -> i64 {
    SystemClock.now_ms()
}

pub fn run(ctx: &mut Ctx) -> Result<i32> {
    let dir = crate::infra::paths::data_dir(ctx.env)?;
    let Some(_lock) = lock::try_acquire(&dir)? else {
        return say(ctx, m::SYNC_RUNNING, 0);
    };
    let mut account = open(ctx)?;
    let connection = match account::load(&account)? {
        Loaded::NotConnected => return say(ctx, m::NOT_CONNECTED, 1),
        Loaded::Broken(text) => return say(ctx, &text, 1),
        Loaded::Ready(c) => c,
    };
    let mut tally = Tally::default();
    collect(ctx, &mut account.store, &mut tally)?;
    // `Some` is a run that ended early: what to say is already on the screen.
    if let Some(code) = upload(ctx, &mut account.store, &connection, &mut tally)? {
        return Ok(code);
    }
    account.store.sync_set("lastSyncAt", &now().to_string())?;
    summary(ctx, &tally)?;
    Ok(0)
}

// --- Reading ---------------------------------------------------------------------------

fn collect(ctx: &mut Ctx, store: &mut Store, tally: &mut Tally) -> Result<()> {
    let at = now();
    let mut any_on = false;
    for source in BOTH {
        let facts = store.source_facts(source)?;
        if facts.enabled_since.is_none() {
            continue;
        }
        any_on = true;
        let outcome = {
            let state = store.reader_state(source)?;
            match source {
                Source::ClaudeCode => claude_code::Config::from_env(ctx.env)
                    .map(|c| claude_code::read(&c, &state, at)),
                Source::Codex => {
                    codex::Config::from_env(ctx.env).map(|c| codex::read(&c, &state, at))
                }
            }
        };
        let Some(outcome) = outcome else { continue };
        match store.commit_read(&outcome, SpoolLimits::default())? {
            Commit::Stored => tally.collected += outcome.observations.len() as u64,
            Commit::Full => {
                // Nothing was saved, so the same data is read again once there is room. The
                // time it covers is a gap, not zero spending.
                let last = facts.last_read_at.as_deref().and_then(Iso::parse);
                let gap = rules::full_gap(source, last, at);
                store.add_pending_coverage(&[gap])?;
                writeln!(
                    ctx.out,
                    "{}: the local buffer is full; reading resumes when it has room.",
                    render::agent_name(source)
                )?;
            }
        }
    }
    if !any_on {
        writeln!(ctx.out, "{}", m::NO_SOURCE_ON)?;
    }
    Ok(())
}

// --- Sending ---------------------------------------------------------------------------

/// A call to the server, tried again while the trouble looks passing. `Err(Final)` is what is
/// left when it is not.
enum Final {
    /// Not passing: the caller decides (`401`, `400`, `413`...).
    Failure(ApiFailure),
    /// Passing, but not this run: `nextAttemptAt` is saved and the message is on the screen.
    Deferred,
}

fn with_retries<T>(
    ctx: &mut Ctx,
    store: &Store,
    mut call: impl FnMut() -> std::result::Result<T, ApiFailure>,
) -> Result<std::result::Result<T, Final>> {
    let mut tries = 0;
    loop {
        match call() {
            Ok(value) => return Ok(Ok(value)),
            Err(f) if rules::classify(&f) == Failed::Transient => {
                tries += 1;
                let failures = store
                    .sync_get("failures")?
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(0)
                    .saturating_add(1);
                store.sync_set("failures", &failures.to_string())?;
                let wait = rules::wait_after(failures, &f, random());
                if wait <= rules::MAX_SLEEP && tries < TRIES_PER_RUN {
                    (ctx.sleep)(wait);
                    continue;
                }
                let next = Iso::from_millis(ms() + wait.as_millis() as i64).unwrap_or(Iso::EPOCH);
                store.sync_set("nextAttemptAt", &next.to_string())?;
                let (waiting, _) = store.pending_counts()?;
                say(
                    ctx,
                    &format!(
                        "{} {waiting} items remain buffered for the next upload.",
                        m::failure_text(&f, ms())
                    ),
                    1,
                )?;
                return Ok(Err(Final::Deferred));
            }
            Err(f) => return Ok(Err(Final::Failure(f))),
        }
    }
}

/// The run stops here, the buffer stays, and the reason is kept for `status`.
fn stop(ctx: &mut Ctx, store: &Store, f: &ApiFailure) -> Result<Option<i32>> {
    if crate::api::is_plain_code(&f.code) {
        store.sync_set("lastError", &f.code)?;
    }
    say(ctx, &m::failure_text(f, ms()), 1).map(Some)
}

fn upload(
    ctx: &mut Ctx,
    store: &mut Store,
    c: &Connection,
    tally: &mut Tally,
) -> Result<Option<i32>> {
    if let Some(next) = store
        .sync_get("nextAttemptAt")?
        .and_then(|t| Iso::parse(&t))
        .filter(|t| t.millis() > ms())
    {
        let (waiting, _) = store.pending_counts()?;
        writeln!(
            ctx.out,
            "Upload deferred until {next}. {waiting} items remain buffered."
        )?;
        return Ok(Some(0));
    }
    let (waiting, coverage) = store.pending_counts()?;
    if waiting + coverage == 0 {
        return Ok(None);
    }
    // What the company authorized now, and from when. Nothing outside it leaves the machine.
    let view = match with_retries(ctx, store, || status_of(c))? {
        Ok(v) => v,
        Err(Final::Deferred) => return Ok(Some(1)),
        Err(Final::Failure(f)) => return stop(ctx, store, &f),
    };
    let mut window: BTreeMap<&'static str, Iso> = BTreeMap::new();
    let mut active: Vec<&'static str> = Vec::new();
    for source in BOTH {
        let granted = view
            .sources
            .iter()
            .find(|s| s.source == source && s.state == SourceState::Active);
        if let Some(s) = granted {
            active.push(source.name());
            if let Some(w) = s.window_start {
                window.insert(source.name(), w);
            }
        } else {
            let held = store.queued_count(source.name())?;
            if held > 0 {
                tally.held.insert(source.name(), held);
            }
        }
    }
    let installation = uuid::Uuid::parse_str(&store.installation_id()?)?;
    let signer = Signer {
        device: &c.device,
        key: &c.key,
    };
    let client = Client::new(c.server.clone(), VERSION);
    let end = store.queue_end()?;
    let mut max_items = crate::contract::MAX_BATCH_OBSERVATIONS;
    // Every pass removes or sends something, so this is never reached; it is a stop for a
    // process that holds the lock if that ever stops being true.
    for _ in 0..MAX_PASSES {
        let rows = store.queued(&active, end, max_items)?;
        let (mut items, invalid) =
            rules::parse_items(rows.into_iter().map(|q| (q.id, q.payload, q.queued_at)));
        let (reports, bad_coverage) = rules::parse_coverage(
            store.queued_coverage(&active, crate::contract::MAX_BATCH_COVERAGE)?,
        );
        // Housekeeping that needs no answer: what is broken, and what is before the window.
        let mut local = Change {
            remove_items: invalid.clone(),
            remove_coverage: bad_coverage,
            ..Change::default()
        };
        let (mut broken, mut early) = (invalid.len() as u64, 0u64);
        items.retain(|item| {
            let name = item.observation.source.name();
            if window
                .get(name)
                .is_some_and(|w| rules::before_window(item, *w))
            {
                local.remove_items.push(item.id);
                early += 1;
                false
            } else {
                true
            }
        });
        let packed = rules::pack(installation, &items, &reports, max_items);
        if packed.is_none() && !items.is_empty() {
            // One observation alone is over the size limit: it can never be sent.
            local.remove_items.push(items[0].id);
            broken += 1;
        }
        for (code, n) in [
            ("INVALID_LOCAL", broken),
            ("BEFORE_COLLECTION_WINDOW", early),
        ] {
            if n > 0 {
                local.rejected.push((code.into(), n));
                tally.add(code, n);
            }
        }
        let removed = !local.remove_items.is_empty() || !local.remove_coverage.is_empty();
        if removed {
            store.apply(&local)?;
        }
        let Some(packed) = packed else {
            if removed {
                continue; // what was behind the removed rows is next
            }
            return Ok(None);
        };
        let sent_items = &items[..packed.items];
        let sent_coverage: Vec<i64> = reports[..packed.coverage]
            .iter()
            .map(|(id, _)| *id)
            .collect();
        let answer = with_retries(ctx, store, || {
            client.post_json_gzip::<IngestResults>(INGEST, &packed.json, Some(&signer))
        })?;
        let raw = match answer {
            Ok(raw) => raw,
            Err(Final::Deferred) => return Ok(Some(1)),
            Err(Final::Failure(f)) => match rules::classify(&f) {
                Failed::DropBatch => {
                    let n = sent_items.len() as u64;
                    tally.add("VALIDATION_FAILED", n);
                    store.apply(&Change {
                        remove_items: sent_items.iter().map(|i| i.id).collect(),
                        remove_coverage: sent_coverage,
                        rejected: vec![("VALIDATION_FAILED".into(), n)],
                        ..Change::default()
                    })?;
                    dropped_batch(ctx, store)?;
                    continue;
                }
                Failed::Split if packed.items > 1 => {
                    max_items = packed.items.div_ceil(2);
                    continue;
                }
                Failed::Split => {
                    // One observation the server finds too big: it can never be sent.
                    tally.add("INVALID_LOCAL", 1);
                    store.apply(&Change {
                        remove_items: vec![sent_items[0].id],
                        rejected: vec![("INVALID_LOCAL".into(), 1)],
                        ..Change::default()
                    })?;
                    continue;
                }
                _ => return stop(ctx, store, &f),
            },
        };
        let Some(outcomes) = raw.checked(packed.items) else {
            // Not the answer the contract promises: the buffer stays, nothing is guessed.
            return stop(ctx, store, &ApiFailure::bad_response(200));
        };
        let applied = rules::apply_results(sent_items, &outcomes, ms());
        let requeue: Vec<Queued> = sent_items
            .iter()
            .filter(|i| applied.requeue.contains(&i.id))
            .map(|i| Queued {
                id: i.id,
                payload: i.payload.clone(),
                queued_at: i.queued_at,
            })
            .collect();
        store.apply(&Change {
            remove_items: applied.delete.clone(),
            requeue,
            remove_coverage: sent_coverage,
            rejected: applied
                .rejected
                .iter()
                .map(|(c, n)| (c.clone(), *n))
                .collect(),
            discard_session: applied.session_owned.clone(),
        })?;
        for (code, n) in &applied.rejected {
            tally.add(code, *n);
        }
        tally.accepted += applied.accepted;
        tally.sent_any = true;
        store.sync_delete(&["failures", "nextAttemptAt", "lastError"])?;
        store.sync_set("lastSentAt", &now().to_string())?;
        if applied.session_owned.is_some() {
            // A session that is not this member's: its buffer is gone, and the run stops here.
            summary(ctx, tally)?;
            return say(ctx, m::SESSION_OWNED_STOP, 1).map(Some);
        }
    }
    Ok(None)
}

/// `Dropped N batch(es) so far`, said on the first drop and then every 50th.
fn dropped_batch(ctx: &mut Ctx, store: &Store) -> Result<()> {
    let n = store
        .sync_get("droppedBatches")?
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
        + 1;
    store.sync_set("droppedBatches", &n.to_string())?;
    if m::report_drop(n) {
        writeln!(
            ctx.out,
            "Dropped {n} batch(es) so far: the server said they were out of contract."
        )?;
    }
    Ok(())
}

fn summary(ctx: &mut Ctx, tally: &Tally) -> Result<()> {
    for (code, n) in &tally.rejected {
        if *n > 0 {
            writeln!(ctx.out, "{}", m::rejected_line(code, *n))?;
        }
    }
    for (name, n) in &tally.held {
        let agent = render::agent_name(if *name == "codex" {
            Source::Codex
        } else {
            Source::ClaudeCode
        });
        writeln!(
            ctx.out,
            "{n} {agent} items stay in the buffer: the company has not authorized {agent} collection."
        )?;
    }
    let line = if tally.accepted > 0 {
        format!("Uploaded {} usage events.", tally.accepted)
    } else if tally.sent_any {
        "No events accepted.".to_owned()
    } else {
        "Nothing new to upload.".to_owned()
    };
    writeln!(ctx.out, "{line}")?;
    Ok(())
}
