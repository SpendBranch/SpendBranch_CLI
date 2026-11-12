//! `hook claude-code` / `hook codex <Event>`: what the agents run. Never writes a word, never
//! fails: whatever goes wrong here must not reach the agent. It reads nothing but the event
//! and the session id, and starts a `sync` that outlives it.
use super::{Ctx, Result, account::now};
use crate::contract::{Iso, Source};
use crate::hooks;
use crate::infra::{hook_log, paths, store::Store};

pub(super) fn run(ctx: &mut Ctx, agent: Option<&str>, event: Option<&str>) -> Result<i32> {
    // Only the spellings the hook lines write: an agent never says `claude_code`.
    let Some(source) = [Source::ClaudeCode, Source::Codex]
        .into_iter()
        .find(|s| agent == Some(s.arg()))
    else {
        return Ok(2);
    };
    let _ = fire(ctx, source, event, now());
    Ok(0)
}

fn fire(ctx: &Ctx, source: Source, argv_event: Option<&str>, at: Iso) -> Result<()> {
    let dir = paths::data_dir(ctx.env)?;
    let (event, session) = match source {
        // Codex's stdin carries the last answer of the assistant: it is never read.
        Source::Codex => (
            Some(format!(
                "codex:{}",
                argv_event
                    .and_then(hooks::plain)
                    .unwrap_or_else(|| "?".into())
            )),
            None,
        ),
        Source::ClaudeCode => ctx
            .system
            .stdin_brief()
            .map_or((None, None), |bytes| hooks::parse_input(&bytes)),
    };
    let label = event.clone().unwrap_or_else(|| "unknown".into());
    let _ = hook_log::append(&dir, &hooks::log_line(at, &label, session.as_deref()));
    // Nothing to send before the first login, and no reason to create the database here.
    if !Store::exists(&dir) {
        return Ok(());
    }
    let store = Store::open(&dir)?;
    if !store.is_connected()? {
        return Ok(());
    }
    let last = store.sync_get("lastSyncAt")?.and_then(|t| Iso::parse(&t));
    let bare = event
        .as_deref()
        .map(|e| e.strip_prefix("codex:").unwrap_or(e));
    if hooks::due(bare, last, at) {
        ctx.system
            .spawn_detached(&ctx.system.exe()?, &["sync"], &hook_log::path(&dir))?;
    }
    Ok(())
}
