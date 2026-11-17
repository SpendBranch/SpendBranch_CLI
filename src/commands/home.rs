//! What the start screen shows. Local state only: no network, no keychain, and the data
//! folder is never created just to look at it.
use super::{Ctx, Result, account, hooks};
use crate::cli::render;
use crate::cli::screen::{Home, HomeAgent};
use crate::contract::{Iso, Source};
use crate::hooks::State;
use crate::infra::{paths, store::Store};
use std::path::Path;

pub fn home(ctx: &Ctx) -> Home {
    match paths::data_dir(ctx.env) {
        Ok(dir) if Store::exists(&dir) => read(ctx, &dir).unwrap_or(Home::Unreadable),
        _ => Home::NotConnected,
    }
}

fn read(ctx: &Ctx, dir: &Path) -> Result<Home> {
    let store = Store::open(dir)?;
    if !store.is_connected()? {
        return Ok(Home::NotConnected);
    }
    let mut agents = Vec::new();
    for source in [Source::ClaudeCode, Source::Codex] {
        agents.push(HomeAgent {
            name: render::agent_name(source),
            arg: source.arg(),
            reading: store.source_enabled(source)?,
            hooked: hooks::view(ctx, source).state == State::Installed,
        });
    }
    let now = account::now();
    Ok(Home::Connected {
        organization: store.meta_get("org_name")?.unwrap_or_default(),
        agents,
        waiting: store.pending_counts()?.0,
        last_sent: store
            .sync_get("lastSentAt")?
            .as_deref()
            .and_then(Iso::parse)
            .map(|at| render::ago(at, now)),
    })
}
