//! The reading half of `sync`: the sources that are on, into the buffer.
use super::super::account::now;
use super::super::{Ctx, Result};
use super::{BOTH, Tally};
use crate::cli::{messages as m, render};
use crate::contract::{Iso, Source};
use crate::infra::store::{Commit, SpoolLimits, Store};
use crate::readers::{claude_code, codex};
use crate::sync::{self as rules};

pub(super) fn collect(ctx: &mut Ctx, store: &mut Store, tally: &mut Tally) -> Result<()> {
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
