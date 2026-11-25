//! The sending half of `sync`: batches, answers, retries and what each answer does to the buffer.
use super::super::account::{Connection, now, status_of};
use super::super::{Ctx, Result, say};
use super::report::{dropped_item, summary};
use super::{BOTH, INGEST, MAX_PASSES, Tally, ms, random};
use crate::api::{ApiFailure, IngestResults, SourceState, StatusView};
use crate::cli::{VERSION, messages as m};
use crate::contract::{CoverageReport, Iso};
use crate::infra::{
    http::{Client, Signer},
    store::{Change, Queued, Store},
};
use crate::sync::{self as rules, Failed, Narrow, TRIES_PER_RUN, Unpacked};
use std::collections::BTreeMap;

/// A call to the server, tried again while the trouble looks passing. `Err(Final)` is what is
/// left when it is not.
enum Final {
    /// Not passing: the caller decides (`401`, `400`, `413`...).
    Failure(ApiFailure),
    /// Passing, but not this run: `nextAttemptAt` is saved and the message is on the screen.
    Deferred,
    /// The server keeps failing (5xx) on this batch: it is narrowed instead of waited for.
    Narrow(ApiFailure),
}

/// `strike` is told of each server error (5xx) and says whether the batch should be narrowed
/// now instead of waited for.
fn with_retries<T>(
    ctx: &mut Ctx,
    store: &Store,
    mut call: impl FnMut() -> std::result::Result<T, ApiFailure>,
    strike: &mut dyn FnMut(&ApiFailure) -> Result<bool>,
) -> Result<std::result::Result<T, Final>> {
    let mut tries = 0;
    loop {
        match call() {
            Ok(value) => {
                // The server answered: whatever failed before is over, and the backoff starts again.
                if store.sync_get("failures")?.is_some() {
                    store.sync_delete(&["failures"])?;
                }
                return Ok(Ok(value));
            }
            Err(f) if rules::classify(&f) == Failed::Transient => {
                tries += 1;
                let failures = store
                    .sync_get("failures")?
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(0)
                    .saturating_add(1);
                store.sync_set("failures", &failures.to_string())?;
                if f.status >= 500 && strike(&f)? {
                    return Ok(Err(Final::Narrow(f)));
                }
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

/// What the company authorizes now: which sources are on, and from when each one counts.
struct Granted {
    window: BTreeMap<&'static str, Iso>,
    active: Vec<&'static str>,
}

/// Nothing outside it leaves the machine; what a source not granted holds stays in the buffer.
fn granted(store: &Store, view: &StatusView, tally: &mut Tally) -> Result<Granted> {
    let mut granted = Granted {
        window: BTreeMap::new(),
        active: Vec::new(),
    };
    for source in BOTH {
        let found = view
            .sources
            .iter()
            .find(|s| s.source == source && s.state == SourceState::Active);
        if let Some(s) = found {
            granted.active.push(source.name());
            if let Some(w) = s.window_start {
                granted.window.insert(source.name(), w);
            }
        } else {
            let held = store.queued_count(source.name())?;
            if held > 0 {
                tally.held.insert(source.name(), held);
            }
        }
    }
    Ok(granted)
}

/// The next batch, after the housekeeping that needs no answer from the server.
struct Next {
    items: Vec<rules::Item>,
    reports: Vec<(i64, CoverageReport)>,
    /// `Err` when there is nothing to send (or one item alone is too big and was removed), or
    /// no batch can be built.
    packed: std::result::Result<rules::Packed, Unpacked>,
    /// Rows left the buffer without being sent.
    removed: bool,
}

/// Reads the oldest rows, drops what is broken or from before the collection window (counted,
/// never sent), and packs what is left.
fn next_batch(
    store: &mut Store,
    granted: &Granted,
    installation: uuid::Uuid,
    end: i64,
    max_items: usize,
    strikes: Option<rules::Strikes>,
    tally: &mut Tally,
) -> Result<Next> {
    let rows = store.queued(&granted.active, end, max_items)?;
    let (mut items, invalid) =
        rules::parse_items(rows.into_iter().map(|q| (q.id, q.payload, q.queued_at)));
    let (reports, bad_coverage) = rules::parse_coverage(
        store.queued_coverage(&granted.active, crate::contract::MAX_BATCH_COVERAGE)?,
    );
    let mut local = Change {
        remove_items: invalid.clone(),
        remove_coverage: bad_coverage,
        ..Change::default()
    };
    let mut broken = invalid.len() as u64;
    let early = drop_before_window(&mut items, granted, &mut local);
    let mut limit = rules::Strikes::limit(strikes, items.first().map(|i| i.id), max_items);
    if limit == 0 && reports.is_empty() {
        // The coverage that was to go alone is gone (sent, or its source no longer active).
        limit = crate::contract::MAX_BATCH_OBSERVATIONS;
    }
    let packed = rules::pack(installation, &items, &reports, limit);
    if packed.as_ref().err() == Some(&Unpacked::TooBig) && !items.is_empty() {
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
    Ok(Next {
        items,
        reports,
        packed,
        removed,
    })
}

/// What is from before its source's collection window is never sent: it is taken out of the
/// batch and queued for removal. Returns how many.
fn drop_before_window(items: &mut Vec<rules::Item>, granted: &Granted, local: &mut Change) -> u64 {
    let mut early = 0;
    items.retain(|item| {
        let name = item.observation.source.name();
        let before = granted
            .window
            .get(name)
            .is_some_and(|w| rules::before_window(item, *w));
        if before {
            local.remove_items.push(item.id);
            early += 1;
        }
        !before
    });
    early
}

/// What was in the batch that went out.
struct Sent<'a> {
    items: &'a [rules::Item],
    coverage: Vec<i64>,
}

/// What to do after a batch the server refused.
enum Flow {
    /// Go round again (the batch left the buffer, or will be smaller).
    Again,
    /// Leave `upload` with this exit code.
    Stop(Option<i32>),
}

fn refused(
    ctx: &mut Ctx,
    store: &mut Store,
    tally: &mut Tally,
    f: &ApiFailure,
    sent: Sent,
    max_items: &mut usize,
) -> Result<Flow> {
    // What is dropped once it is alone: an observation the server finds too big can never be
    // sent; one it calls out of contract is the one that spoiled the batch.
    let code = match rules::classify(f) {
        Failed::OutOfContract => "VALIDATION_FAILED",
        Failed::Split => "INVALID_LOCAL",
        _ => return Ok(Flow::Stop(stop(ctx, store, f)?)),
    };
    let full = crate::contract::MAX_BATCH_OBSERVATIONS;
    match rules::narrow(sent.items.len(), sent.coverage.len()) {
        Narrow::CoverageAlone => *max_items = 0,
        Narrow::Halve(n) => *max_items = n,
        Narrow::DropItem => {
            tally.add(code, 1);
            store.apply(&Change {
                remove_items: vec![sent.items[0].id],
                rejected: vec![(code.into(), 1)],
                ..Change::default()
            })?;
            if code == "VALIDATION_FAILED" {
                dropped_item(ctx, store)?;
            }
            *max_items = full;
        }
        // Coverage alone the server refuses: it goes, or every run would fail on it again.
        Narrow::DropCoverage => {
            store.apply(&Change {
                remove_coverage: sent.coverage,
                ..Change::default()
            })?;
            *max_items = full;
        }
    }
    Ok(Flow::Again)
}

/// The batch the server keeps failing on (`Strikes`, as text).
const STRIKES: &str = "serverStrikes";
/// The items the server kept failing on even alone, by `item_key`, with its code.
const STUCK: &str = "stuckItems";

/// The server failed on the batch again and again: it is narrowed like a refused one, but
/// nothing is dropped. An item that fails alone goes back to the end of the queue, so the rest
/// can go, and `status` says it is there.
fn server_failing(
    store: &mut Store,
    tally: &mut Tally,
    f: &ApiFailure,
    sent: Sent,
) -> Result<Pass> {
    let Some(head) = sent.items.first() else {
        return Ok(Pass::Done(Some(1)));
    };
    let narrowed = |size| rules::Strikes {
        head: head.id,
        size,
        count: 0,
    };
    match rules::narrow(sent.items.len(), sent.coverage.len()) {
        Narrow::CoverageAlone => store.sync_set(STRIKES, &narrowed(0).text())?,
        Narrow::Halve(n) => store.sync_set(STRIKES, &narrowed(n).text())?,
        Narrow::DropItem | Narrow::DropCoverage => {
            store.apply(&Change {
                requeue: vec![Queued {
                    id: head.id,
                    payload: head.payload.clone(),
                    queued_at: head.queued_at,
                }],
                ..Change::default()
            })?;
            let code = if crate::api::is_plain_code(&f.code) {
                f.code.as_str()
            } else {
                "UNAVAILABLE"
            };
            let stuck = store.sync_get(STUCK)?;
            if let Some(list) = rules::stuck_add(stuck.as_deref(), rules::item_key(head), code) {
                store.sync_set(STUCK, &list)?;
            }
            store.sync_delete(&[STRIKES])?;
            tally.stuck += 1;
        }
    }
    Ok(Pass::Next)
}

/// Applies the server's answer to the buffer and to the numbers of the run, in one transaction.
fn record(
    store: &mut Store,
    tally: &mut Tally,
    sent: Sent,
    applied: &rules::Applied,
) -> Result<()> {
    let requeue: Vec<Queued> = sent
        .items
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
        remove_coverage: sent.coverage.clone(),
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
    store.sync_delete(&["failures", "nextAttemptAt", "lastError", STRIKES])?;
    let stuck = store.sync_get(STUCK)?;
    if stuck.is_some() {
        match rules::stuck_remove(stuck.as_deref(), sent.items.iter().map(rules::item_key)) {
            Some(left) => store.sync_set(STUCK, &left)?,
            None => store.sync_delete(&[STUCK])?,
        }
    }
    store.sync_set("lastSentAt", &now().to_string())?;
    Ok(())
}

/// What one upload run holds on to while it goes batch by batch.
struct Run<'a> {
    granted: Granted,
    installation: uuid::Uuid,
    signer: Signer<'a>,
    client: Client,
    /// Rows put back during this run are past this and wait for the next one.
    end: i64,
}

/// How a batch went.
enum Pass {
    Next,
    Done(Option<i32>),
}

/// `true` (after saying so) while `nextAttemptAt` is still in the future. One further out than
/// any server may ask for was saved under a clock that ran ahead: it is forgotten, or uploads
/// would stop for as long as the clock was wrong.
fn deferred(ctx: &mut Ctx, store: &Store) -> Result<bool> {
    let now = ms();
    let Some(next) = store
        .sync_get("nextAttemptAt")?
        .and_then(|t| Iso::parse(&t))
        .filter(|t| t.millis() > now)
    else {
        return Ok(false);
    };
    if next.millis() - now > rules::MAX_DEFER_MS {
        store.sync_delete(&["nextAttemptAt"])?;
        return Ok(false);
    }
    let (waiting, _) = store.pending_counts()?;
    writeln!(
        ctx.out,
        "Upload deferred until {next}. {waiting} items remain buffered."
    )?;
    Ok(true)
}

pub(super) fn upload(
    ctx: &mut Ctx,
    store: &mut Store,
    c: &Connection,
    tally: &mut Tally,
) -> Result<Option<i32>> {
    if deferred(ctx, store)? {
        return Ok(Some(0));
    }
    let (waiting, coverage) = store.pending_counts()?;
    if waiting + coverage == 0 {
        store.sync_delete(&[STUCK, STRIKES])?;
        return Ok(None);
    }
    let Some(installation) = uuid::Uuid::parse_str(&store.installation_id()?)
        .ok()
        .filter(|id| rules::installation_ok(*id))
    else {
        return say(ctx, m::BAD_INSTALLATION_ID, 1).map(Some);
    };
    // What the company authorized now, and from when.
    let view = match with_retries(ctx, store, || status_of(c), &mut |_| Ok(false))? {
        Ok(v) => v,
        Err(Final::Deferred) => return Ok(Some(1)),
        Err(Final::Failure(f) | Final::Narrow(f)) => return stop(ctx, store, &f),
    };
    let run = Run {
        granted: granted(store, &view, tally)?,
        installation,
        signer: Signer {
            device: &c.device,
            key: &c.key,
        },
        client: Client::new(c.server.clone(), VERSION),
        end: store.queue_end()?,
    };
    let mut max_items = crate::contract::MAX_BATCH_OBSERVATIONS;
    // Every pass removes or sends something, so this is never reached; it is a stop for a
    // process that holds the lock if that ever stops being true.
    for _ in 0..MAX_PASSES {
        match pass(ctx, store, tally, &run, &mut max_items)? {
            Pass::Next => {}
            Pass::Done(code) => return Ok(code),
        }
    }
    Ok(None)
}

/// One batch: pack it, send it, and apply what the server said.
fn pass(
    ctx: &mut Ctx,
    store: &mut Store,
    tally: &mut Tally,
    run: &Run,
    max_items: &mut usize,
) -> Result<Pass> {
    let strikes = store
        .sync_get(STRIKES)?
        .and_then(|t| rules::Strikes::parse(&t));
    let next = next_batch(
        store,
        &run.granted,
        run.installation,
        run.end,
        *max_items,
        strikes,
        tally,
    )?;
    let packed = match next.packed {
        Ok(packed) => packed,
        // Not the fault of any observation: the run stops and the buffer stays.
        Err(Unpacked::Invalid) => {
            return Ok(Pass::Done(say(ctx, m::BAD_INSTALLATION_ID, 1).map(Some)?));
        }
        // What was behind the removed rows is next; with nothing removed, nothing is left.
        Err(_) => {
            return Ok(if next.removed {
                Pass::Next
            } else {
                Pass::Done(None)
            });
        }
    };
    let sent = || Sent {
        items: &next.items[..packed.items],
        coverage: next.reports[..packed.coverage]
            .iter()
            .map(|(id, _)| *id)
            .collect(),
    };
    let head = next
        .items
        .first()
        .map(|i| i.id)
        .filter(|_| packed.items > 0);
    let answer = with_retries(
        ctx,
        store,
        || {
            run.client
                .post_json_gzip::<IngestResults>(INGEST, &packed.json, Some(&run.signer))
        },
        &mut |_| {
            // Only a batch with observations can be narrowed.
            let Some(head) = head else { return Ok(false) };
            let previous = store
                .sync_get(STRIKES)?
                .and_then(|t| rules::Strikes::parse(&t));
            let now = rules::Strikes::add(previous, head, packed.items);
            store.sync_set(STRIKES, &now.text())?;
            Ok(now.count >= rules::SERVER_ERRORS_TO_NARROW)
        },
    )?;
    let raw = match answer {
        Ok(raw) => raw,
        Err(Final::Deferred) => return Ok(Pass::Done(Some(1))),
        Err(Final::Narrow(f)) => return server_failing(store, tally, &f, sent()),
        Err(Final::Failure(f)) => {
            return Ok(match refused(ctx, store, tally, &f, sent(), max_items)? {
                Flow::Again => Pass::Next,
                Flow::Stop(code) => Pass::Done(code),
            });
        }
    };
    let Some(outcomes) = raw.checked(packed.items) else {
        // Not the answer the contract promises: the buffer stays, nothing is guessed.
        return Ok(Pass::Done(stop(
            ctx,
            store,
            &ApiFailure::bad_response(200),
        )?));
    };
    let applied = rules::apply_results(sent().items, &outcomes, ms());
    record(store, tally, sent(), &applied)?;
    // Whatever was cut down to get this batch through (the coverage alone, a half after a
    // refusal) was about this batch: the next one starts full again.
    *max_items = crate::contract::MAX_BATCH_OBSERVATIONS;
    if applied.session_owned.is_some() {
        // A session that is not this member's: its buffer is gone, and the run stops here.
        summary(ctx, tally)?;
        return Ok(Pass::Done(say(ctx, m::SESSION_OWNED_STOP, 1).map(Some)?));
    }
    Ok(Pass::Next)
}
