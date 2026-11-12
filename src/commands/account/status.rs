//! `status`: what is local, then what the server says.
use super::super::{Ctx, Result, say};
use super::{Loaded, fetch_status, load, now, open};
use crate::cli::{messages as m, render};
use crate::contract::Source;
use crate::infra::{keys::Backend, scheduler, store::Store};
use crate::readers::{claude_code, codex};

pub(super) fn folder_of(source: Source, ctx: &Ctx) -> String {
    let folder = match source {
        Source::ClaudeCode => claude_code::Config::from_env(ctx.env).map(|c| c.projects_dir),
        Source::Codex => codex::Config::from_env(ctx.env).map(|c| c.root),
    };
    folder.map_or_else(|| "unknown".into(), |p| p.display().to_string())
}

pub(super) fn local_status(ctx: &Ctx, store: &Store) -> Result<render::Local> {
    let (items, coverage) = store.pending_counts()?;
    let mut sources = Vec::new();
    for source in [Source::ClaudeCode, Source::Codex] {
        sources.push(render::LocalSource {
            source,
            facts: store.source_facts(source)?,
            folder: folder_of(source, ctx),
            hook: super::super::hooks::view(ctx, source),
        });
    }
    Ok(render::Local {
        items,
        coverage,
        rejected: store.rejected_counts()?,
        last_sent: store.sync_get("lastSentAt")?,
        last_error: store.sync_get("lastError")?,
        next_attempt: store.sync_get("nextAttemptAt")?,
        sources,
        scheduled: scheduler::installed(ctx.system, ctx.env, ctx.system.platform()),
    })
}

fn print_lines(ctx: &mut Ctx, lines: Vec<String>) -> Result<()> {
    for line in lines {
        writeln!(ctx.out, "{line}")?;
    }
    Ok(())
}

pub fn status(ctx: &mut Ctx) -> Result<i32> {
    let account = open(ctx)?;
    let local = local_status(ctx, &account.store)?;
    let connection = match load(&account)? {
        Loaded::NotConnected => return say(ctx, m::NOT_CONNECTED, 1),
        Loaded::Broken(text) => {
            say(ctx, &text, 1)?;
            print_lines(ctx, render::local_lines(&local, now()))?;
            return Ok(1);
        }
        Loaded::Ready(c) => c,
    };
    match fetch_status(&connection) {
        Ok(view) => {
            let file = account.keys.backend() == Backend::File;
            let lines = render::remote_lines(
                &view,
                &connection.server.base(),
                &connection.key.fingerprint(),
                file,
                &local,
                now(),
            );
            print_lines(ctx, lines)?;
            print_lines(ctx, render::local_lines(&local, now()))?;
            Ok(0)
        }
        Err(text) => {
            say(ctx, &text, 1)?;
            print_lines(ctx, render::local_lines(&local, now()))?;
            Ok(1)
        }
    }
}
