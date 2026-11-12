//! What `sync` says about the run.
use super::super::{Ctx, Result};
use super::Tally;
use crate::cli::{messages as m, render};
use crate::contract::Source;
use crate::infra::store::Store;

/// `Dropped N batch(es) so far`, said on the first drop and then every 50th.
pub(super) fn dropped_batch(ctx: &mut Ctx, store: &Store) -> Result<()> {
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

pub(super) fn summary(ctx: &mut Ctx, tally: &Tally) -> Result<()> {
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
