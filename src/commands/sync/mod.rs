//! `sync`: read what is new from the sources that are on, then send the buffer in batches.
//! Nothing leaves the buffer until the server answered for it, so a crash loses nothing.
use super::account::{self, Loaded, now, open};
use super::{Ctx, Result, say};
use crate::cli::messages as m;
use crate::contract::Source;
use crate::infra::{
    clock::{Clock, SystemClock},
    lock,
};
use std::collections::BTreeMap;

pub(super) const INGEST: &str = "/api/ingest/v1/events";
pub(super) const MAX_PASSES: usize = 100_000;
pub(super) const BOTH: [Source; 2] = [Source::ClaudeCode, Source::Codex];

/// What this run did, for the last lines.
#[derive(Default)]
pub(super) struct Tally {
    pub(super) collected: u64,
    pub(super) accepted: u64,
    pub(super) sent_any: bool,
    pub(super) rejected: BTreeMap<String, u64>,
    /// Observations of a source the company has not (or no longer) authorized: kept, not sent.
    pub(super) held: BTreeMap<&'static str, u64>,
}

impl Tally {
    pub(super) fn add(&mut self, code: &str, n: u64) {
        *self.rejected.entry(code.to_owned()).or_default() += n;
    }
}

pub(super) fn random() -> f64 {
    let mut bytes = [0u8; 8];
    // Without entropy the jitter is just the middle of the range: a wait is still a wait.
    if getrandom::fill(&mut bytes).is_err() {
        return 0.5;
    }
    u64::from_le_bytes(bytes) as f64 / u64::MAX as f64
}

pub(super) fn ms() -> i64 {
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

mod collect;
mod report;
mod send;

use collect::collect;
use report::summary;
use send::upload;
