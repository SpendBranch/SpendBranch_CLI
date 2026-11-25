//! The rules of sending: what goes in a batch, what an answer means, and how long to wait.
//! Pure: the buffer, the network and the clock are the caller's.
use crate::api::{ApiFailure, ItemOutcome};
use crate::contract::{
    CoverageReport, CoverageState, Encoded, GapReason, IngestBatch, Iso, MAX_BATCH_COVERAGE,
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
/// How many server errors (5xx) in a row the same batch gets before it is narrowed instead of
/// only waiting: as many as one run tries it.
pub const SERVER_ERRORS_TO_NARROW: u32 = TRIES_PER_RUN;
/// A wait longer than this is not slept through: it is saved as `nextAttemptAt`.
pub const MAX_SLEEP: Duration = Duration::from_secs(8);
/// The furthest a saved `nextAttemptAt` may be: the server's longest `Retry-After` (24 h) and
/// a margin. Anything past it is a clock that was wrong when it was saved.
pub const MAX_DEFER_MS: i64 = (crate::api::MAX_RETRY_AFTER_SECS as i64 + 15 * 60) * 1000;

/// A buffered observation that passed the contract again on the way out.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: i64,
    /// Unix ms of when it entered the buffer.
    pub queued_at: i64,
    pub payload: String,
    pub observation: UsageObservation,
    /// The observation as it goes in a batch: written once from the parsed value, so the
    /// contract is checked on the way out without serializing it again on each halving.
    pub encoded: Encoded<UsageObservation>,
}

impl Item {
    pub fn new(id: i64, queued_at: i64, payload: String, observation: UsageObservation) -> Self {
        Self {
            id,
            queued_at,
            payload,
            encoded: Encoded::new(&observation),
            observation,
        }
    }
}

/// Rows from the buffer into items. A row that no longer passes the contract (a hand-edited
/// file, an older build) is returned apart, to be removed and counted.
pub fn parse_items(rows: impl IntoIterator<Item = (i64, String, i64)>) -> (Vec<Item>, Vec<i64>) {
    let (mut items, mut invalid) = (Vec::new(), Vec::new());
    for (id, payload, queued_at) in rows {
        match serde_json::from_str::<UsageObservation>(&payload) {
            Ok(observation) => items.push(Item::new(id, queued_at, payload, observation)),
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

/// Why there is no batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unpacked {
    /// Nothing to send.
    Nothing,
    /// The first observation alone is over the size: it can never be sent.
    TooBig,
    /// The batch cannot be built at all (the installation id is not one the contract takes):
    /// no observation is at fault.
    Invalid,
}

/// Whether this installation id can go in a batch. Checked once before a run, so a bad id
/// stops the run instead of being blamed on the observations.
pub fn installation_ok(installation_id: uuid::Uuid) -> bool {
    crate::contract::standard_installation_id(installation_id)
}

/// The largest batch from the front of the queue that fits the limits: at most 500
/// observations (and `max_items`), 50 coverage reports and 1 MiB of JSON. A batch over the
/// size is halved until it fits.
pub fn pack(
    installation_id: uuid::Uuid,
    items: &[Item],
    coverage: &[(i64, CoverageReport)],
    max_items: usize,
) -> Result<Packed, Unpacked> {
    pack_within(installation_id, items, coverage, max_items, MAX_BODY_BYTES)
}

/// `pack` with the size limit given: the tests use a small one.
pub fn pack_within(
    installation_id: uuid::Uuid,
    items: &[Item],
    coverage: &[(i64, CoverageReport)],
    max_items: usize,
    max_bytes: usize,
) -> Result<Packed, Unpacked> {
    let mut n = items.len().min(MAX_BATCH_OBSERVATIONS).min(max_items);
    let c = coverage.len().min(MAX_BATCH_COVERAGE);
    let observations: Vec<&Encoded<UsageObservation>> =
        items[..n].iter().map(|i| &i.encoded).collect();
    let reports: Vec<Encoded<CoverageReport>> =
        coverage[..c].iter().map(|(_, r)| Encoded::new(r)).collect();
    loop {
        if n + c == 0 {
            return Err(Unpacked::Nothing);
        }
        // The size is worked out from the parts; the bytes are written once, for what fits.
        let len = IngestBatch::encoded_len(installation_id, &observations[..n], &reports)
            .ok_or(Unpacked::Invalid)?;
        if len <= max_bytes {
            let json = IngestBatch::encode(installation_id, &observations[..n], &reports)
                .ok_or(Unpacked::Invalid)?;
            return Ok(Packed {
                items: n,
                coverage: c,
                json,
            });
        }
        if n <= 1 {
            return Err(Unpacked::TooBig);
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
    /// The batch is out of contract (`400 VALIDATION_FAILED`): one bad item refuses the whole
    /// batch, so it is narrowed down until the item at fault is alone (see `narrow`).
    OutOfContract,
    /// `413`: the batch is too big for the server: halve it.
    Split,
    /// Anything else (`401`, `402`, `403`, `415`...): stop, the buffer stays.
    Stop,
}

pub fn classify(failure: &ApiFailure) -> Failed {
    match (failure.status, failure.code.as_str()) {
        (0, _) | (429, _) => Failed::Transient,
        (s, _) if s >= 500 => Failed::Transient,
        (400, "VALIDATION_FAILED") => Failed::OutOfContract,
        (413, _) => Failed::Split,
        _ => Failed::Stop,
    }
}

/// How a refused batch is cut down: nothing leaves the buffer until it is alone in a batch
/// the server still refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Narrow {
    /// The coverage goes alone first, so no event is blamed for it.
    CoverageAlone,
    /// Try again with at most this many observations (and no coverage).
    Halve(usize),
    /// One observation alone was refused: it is the one at fault.
    DropItem,
    /// Coverage alone was refused: it can never go.
    DropCoverage,
}

/// What to do with a batch of `items` observations and `coverage` reports that was refused.
pub fn narrow(items: usize, coverage: usize) -> Narrow {
    match (items, coverage) {
        (1.., 1..) => Narrow::CoverageAlone,
        (2.., 0) => Narrow::Halve(items.div_ceil(2)),
        (1, 0) => Narrow::DropItem,
        _ => Narrow::DropCoverage,
    }
}

/// The batch the server keeps failing on (5xx): the id of its first observation, how many
/// observations it holds, and how many server errors in a row it got. Kept between runs, so a
/// batch is narrowed even when each run gives up after one try.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Strikes {
    pub head: i64,
    pub size: usize,
    pub count: u32,
}

impl Strikes {
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split(':');
        let strikes = Self {
            head: parts.next()?.parse().ok()?,
            size: parts.next()?.parse().ok()?,
            count: parts.next()?.parse().ok()?,
        };
        parts.next().is_none().then_some(strikes)
    }

    pub fn text(&self) -> String {
        format!("{}:{}:{}", self.head, self.size, self.count)
    }

    /// The most observations the batch starting at `head` may hold.
    pub fn limit(this: Option<Self>, head: Option<i64>, max_items: usize) -> usize {
        match this {
            Some(s) if Some(s.head) == head => max_items.min(s.size),
            _ => max_items,
        }
    }

    /// One more server error for the batch that starts at `head` with `size` observations.
    pub fn add(this: Option<Self>, head: i64, size: usize) -> Self {
        let count = match this {
            Some(s) if s.head == head && s.size == size => s.count.saturating_add(1),
            _ => 1,
        };
        Self { head, size, count }
    }
}

/// The key an item is known by in the list of items the server keeps failing on.
pub fn item_key(item: &Item) -> String {
    let o = &item.observation;
    format!(
        "{}/{}/{}",
        o.source.name(),
        o.external_session_id.as_str(),
        o.external_event_id.as_str()
    )
}

type Stuck = BTreeMap<String, String>;

fn stuck_of(text: Option<&str>) -> Stuck {
    text.and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_default()
}

fn stuck_text(stuck: &Stuck) -> Option<String> {
    (!stuck.is_empty())
        .then(|| serde_json::to_string(stuck).ok())
        .flatten()
}

/// The list (JSON, as kept) with this item added, under the server's code.
pub fn stuck_add(text: Option<&str>, key: String, code: &str) -> Option<String> {
    let mut stuck = stuck_of(text);
    stuck.insert(key, code.to_owned());
    stuck_text(&stuck)
}

/// The list without these items: the server answered for them. `None` when it is empty.
pub fn stuck_remove(text: Option<&str>, keys: impl IntoIterator<Item = String>) -> Option<String> {
    let mut stuck = stuck_of(text);
    for key in keys {
        stuck.remove(&key);
    }
    stuck_text(&stuck)
}

/// How many items the server keeps failing on, and the code it gave for one of them.
pub fn stuck_summary(text: Option<&str>) -> Option<(u64, String)> {
    let stuck = stuck_of(text);
    let code = stuck.values().next()?.clone();
    Some((stuck.len() as u64, code))
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

/// How long the answer to "what did the company authorize" is reused before it is asked again.
pub const GRANTS_KEEP_MS: i64 = 10 * 60 * 1000;

/// The refusals of an item that say the answer this device went by is out of date.
pub const GRANT_CODES: [&str; 3] = [
    "COLLECTION_NOT_AUTHORIZED",
    "FIELDS_NOT_AUTHORIZED",
    "BEFORE_COLLECTION_WINDOW",
];

/// Whether an answer saved at `saved_ms` can still be used at `now_ms`. One saved in the future
/// (the clock went back) cannot.
pub fn grants_fresh(saved_ms: i64, now_ms: i64) -> bool {
    (0..GRANTS_KEEP_MS).contains(&(now_ms - saved_ms))
}

fn grant_refusal(result: &ItemOutcome) -> bool {
    matches!(result, ItemOutcome::Rejected(code) if GRANT_CODES.contains(&code.as_str()))
}

/// Whether the server refused an item over what the company authorized.
pub fn grant_refused(results: &[ItemOutcome]) -> bool {
    results.iter().any(grant_refusal)
}

/// The batch went out on a reused answer: what the server refused over the authorization stays
/// in the buffer and is not counted, to be tried again on a fresh answer.
pub fn spare_grant_refusals(items: &[Item], results: &[ItemOutcome], applied: &mut Applied) {
    for (item, result) in items.iter().zip(results) {
        let ItemOutcome::Rejected(code) = result else {
            continue;
        };
        if !grant_refusal(result) {
            continue;
        }
        applied.delete.retain(|id| *id != item.id);
        if let Some(n) = applied.rejected.get_mut(code) {
            *n -= 1;
            if *n == 0 {
                applied.rejected.remove(code);
            }
        }
    }
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
