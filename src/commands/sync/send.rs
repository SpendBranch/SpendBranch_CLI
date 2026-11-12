//! The sending half of `sync`: batches, answers, retries and what each answer does to the buffer.
use super::super::account::{Connection, now, status_of};
use super::super::{Ctx, Result, say};
use super::report::{dropped_batch, summary};
use super::{BOTH, INGEST, MAX_PASSES, Tally, ms, random};
use crate::api::{ApiFailure, IngestResults, SourceState};
use crate::cli::{VERSION, messages as m};
use crate::contract::Iso;
use crate::infra::{
    http::{Client, Signer},
    store::{Change, Queued, Store},
};
use crate::sync::{self as rules, Failed, TRIES_PER_RUN};
use std::collections::BTreeMap;

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

pub(super) fn upload(
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
