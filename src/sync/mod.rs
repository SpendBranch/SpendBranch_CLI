//! The rules of sending: what goes in a batch, what an answer means, and how long to wait.
//! Pure: the buffer, the network and the clock are the caller's.
use crate::api::{ApiFailure, ItemOutcome};
use crate::contract::{
    CoverageReport, CoverageState, GapReason, IngestBatch, Iso, MAX_BATCH_COVERAGE,
    MAX_BATCH_OBSERVATIONS, Source, UsageObservation,
};
use std::{collections::BTreeMap, time::Duration};

/// An item the server calls `CLOCK_SKEW` (its time is ahead of the server's) is tried again
/// until it has been in the buffer this long, and then dropped.
pub const CLOCK_SKEW_CAP_MS: i64 = 24 * 60 * 60 * 1000;
/// The JSON of a batch, before gzip, is never larger than this.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// How often the same batch is tried within one run.
pub const TRIES_PER_RUN: u32 = 3;
/// A wait longer than this is not slept through: it is saved as `nextAttemptAt`.
pub const MAX_SLEEP: Duration = Duration::from_secs(8);

/// A buffered observation that passed the contract again on the way out.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: i64,
    /// Unix ms of when it entered the buffer.
    pub queued_at: i64,
    pub payload: String,
    pub observation: UsageObservation,
}

/// Rows from the buffer into items. A row that no longer passes the contract (a hand-edited
/// file, an older build) is returned apart, to be removed and counted.
pub fn parse_items(rows: impl IntoIterator<Item = (i64, String, i64)>) -> (Vec<Item>, Vec<i64>) {
    let (mut items, mut invalid) = (Vec::new(), Vec::new());
    for (id, payload, queued_at) in rows {
        match serde_json::from_str::<UsageObservation>(&payload) {
            Ok(observation) => items.push(Item {
                id,
                queued_at,
                payload,
                observation,
            }),
            Err(_) => invalid.push(id),
        }
    }
    (items, invalid)
}

pub fn parse_coverage(
    rows: impl IntoIterator<Item = (i64, String)>,
) -> (Vec<(i64, CoverageReport)>, Vec<i64>) {
    let (mut reports, mut invalid) = (Vec::new(), Vec::new());
    for (id, payload) in rows {
        match serde_json::from_str::<CoverageReport>(&payload) {
            Ok(report) => reports.push((id, report)),
            Err(_) => invalid.push(id),
        }
    }
    (reports, invalid)
}

/// A batch ready to send.
#[derive(Debug)]
pub struct Packed {
    /// How many of the items (from the front) are in it.
    pub items: usize,
    pub coverage: usize,
    pub json: Vec<u8>,
}

/// The largest batch from the front of the queue that fits the limits: at most 500
/// observations (and `max_items`), 50 coverage reports and 1 MiB of JSON. A batch over the
/// size is halved until it fits. `None` when there is nothing to send, or one observation
/// alone is over the size.
pub fn pack(
    installation_id: uuid::Uuid,
    items: &[Item],
    coverage: &[(i64, CoverageReport)],
    max_items: usize,
) -> Option<Packed> {
    pack_within(installation_id, items, coverage, max_items, MAX_BODY_BYTES)
}

/// `pack` with the size limit given: the tests use a small one.
pub fn pack_within(
    installation_id: uuid::Uuid,
    items: &[Item],
    coverage: &[(i64, CoverageReport)],
    max_items: usize,
    max_bytes: usize,
) -> Option<Packed> {
    let mut n = items.len().min(MAX_BATCH_OBSERVATIONS).min(max_items);
    let c = coverage.len().min(MAX_BATCH_COVERAGE);
    loop {
        if n + c == 0 {
            return None;
        }
        let batch = IngestBatch::new(
            installation_id,
            items[..n].iter().map(|i| i.observation.clone()).collect(),
            coverage[..c].iter().map(|(_, r)| r.clone()).collect(),
        )?;
        let json = serde_json::to_vec(&batch).ok()?;
        if json.len() <= max_bytes {
            return Some(Packed {
                items: n,
                coverage: c,
                json,
            });
        }
        if n <= 1 {
            return None;
        }
        n = n.div_ceil(2);
    }
}

/// `1 s × 2^(failures-1)`, at most 15 minutes, then somewhere between half and all of it
/// (`random` is 0.0 to 1.0): a crowd of devices does not come back at the same second.
pub fn backoff(failures: u32, random: f64) -> Duration {
    let exponent = failures.saturating_sub(1).min(20);
    let base = Duration::from_secs(1 << exponent).min(Duration::from_secs(15 * 60));
    base.mul_f64(0.5 + 0.5 * random.clamp(0.0, 1.0))
}

/// What to do about a failed call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failed {
    /// The network, a throttle or a server that stumbled: wait and try the same batch again.
    Transient,
    /// The whole batch is out of contract (`400 VALIDATION_FAILED`): drop it, go on.
    DropBatch,
    /// `413`: the batch is too big for the server: halve it.
    Split,
    /// Anything else (`401`, `402`, `403`, `415`...): stop, the buffer stays.
    Stop,
}

pub fn classify(failure: &ApiFailure) -> Failed {
    match (failure.status, failure.code.as_str()) {
        (0, _) | (429, _) => Failed::Transient,
        (s, _) if s >= 500 => Failed::Transient,
        (400, "VALIDATION_FAILED") => Failed::DropBatch,
        (413, _) => Failed::Split,
        _ => Failed::Stop,
    }
}

/// How long to wait before the next try: the backoff, or what the server asked for, if more.
pub fn wait_after(failures: u32, failure: &ApiFailure, random: f64) -> Duration {
    backoff(failures, random).max(Duration::from_secs(failure.retry_after_secs.unwrap_or(0)))
}

/// What one answer changes in the buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub accepted: u64,
    pub duplicate: u64,
    /// Ids of items that leave the buffer.
    pub delete: Vec<i64>,
    /// Ids that go back to the end of the queue (`CLOCK_SKEW`, not too old yet).
    pub requeue: Vec<i64>,
    /// By code; local codes (`CLOCK_SKEW_EXPIRED`) are in here too.
    pub rejected: BTreeMap<String, u64>,
    /// The device was revoked over this (source, external session id): its buffer goes too.
    pub session_owned: Option<(String, String)>,
}

pub fn apply_results(items: &[Item], results: &[ItemOutcome], now_ms: i64) -> Applied {
    let mut applied = Applied::default();
    for (item, result) in items.iter().zip(results) {
        match result {
            ItemOutcome::Accepted => {
                applied.accepted += 1;
                applied.delete.push(item.id);
            }
            ItemOutcome::Duplicate => {
                applied.duplicate += 1;
                applied.delete.push(item.id);
            }
            ItemOutcome::Rejected(code) if code == "CLOCK_SKEW" => {
                if now_ms - item.queued_at > CLOCK_SKEW_CAP_MS {
                    applied.delete.push(item.id);
                    *applied
                        .rejected
                        .entry("CLOCK_SKEW_EXPIRED".into())
                        .or_default() += 1;
                } else {
                    applied.requeue.push(item.id);
                }
            }
            ItemOutcome::Rejected(code) => {
                applied.delete.push(item.id);
                *applied.rejected.entry(code.clone()).or_default() += 1;
                if code == "SESSION_OWNED" {
                    applied.session_owned = Some((
                        item.observation.source.name().to_owned(),
                        item.observation.external_session_id.as_str().to_owned(),
                    ));
                }
            }
        }
    }
    applied
}

/// An item from before the start of the authorized window is not sent: the server would only
/// refuse it.
pub fn before_window(item: &Item, window_start: Iso) -> bool {
    item.observation.occurred_at < window_start
}

/// The gap to record when the buffer had no room and a read was refused: from the last read
/// (or an hour ago when there was none, or it is not before now) until now. What it covers
/// was not read, and that is a gap, never zero spending.
pub fn full_gap(source: Source, last_read: Option<Iso>, now: Iso) -> CoverageReport {
    let hour_ago = Iso::from_millis((now.millis() - 3_600_000).max(0)).unwrap_or(now);
    let from = last_read.filter(|t| *t < now).unwrap_or(hour_ago);
    CoverageReport::new(
        source,
        from,
        now,
        CoverageState::Unavailable {
            reason: GapReason::BufferFull,
        },
    )
    .unwrap_or_else(|| CoverageReport::unavailable_ending(source, now, GapReason::BufferFull))
}
