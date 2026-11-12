//! `enable` / `disable`: which agents are collected.
use super::super::{Ctx, Result, say};
use super::status::folder_of;
use super::{Loaded, fetch_status, load, now, open};
use crate::api::SourceState;
use crate::cli::{messages as m, render};
use crate::contract::Source;
use crate::sources::{claude_code as claude_parser, codex as codex_parser};

fn parser_of(source: Source) -> (&'static str, &'static [&'static str]) {
    match source {
        Source::ClaudeCode => (claude_parser::PARSER, &claude_parser::FIELDS),
        Source::Codex => (codex_parser::PARSER, &codex_parser::FIELDS),
    }
}

pub fn enable(ctx: &mut Ctx, source: Source) -> Result<i32> {
    enable_with(ctx, source, true)
}

/// `hint`: say how to install the hook. `setup` installs it right after, so it asks for none.
pub(in crate::commands) fn enable_with(ctx: &mut Ctx, source: Source, hint: bool) -> Result<i32> {
    let account = open(ctx)?;
    let connection = match load(&account)? {
        Loaded::NotConnected => return say(ctx, m::NOT_CONNECTED, 1),
        Loaded::Broken(text) => return say(ctx, &text, 1),
        Loaded::Ready(c) => c,
    };
    let view = match fetch_status(&connection) {
        Ok(v) => v,
        Err(text) => return say(ctx, &text, 1),
    };
    let agent = render::agent_name(source);
    let Some(granted) = view
        .sources
        .iter()
        .find(|s| s.source == source && s.state == SourceState::Active)
    else {
        return say(ctx, &m::not_authorized(agent), 1);
    };
    // Every field this reader reads must be on the owner's list, the same names in the same
    // order: anything else is refused at upload (`FIELDS_NOT_AUTHORIZED`).
    let (parser, fields) = parser_of(source);
    if granted.parser_version.as_str() != parser
        || !granted
            .fields
            .iter()
            .map(String::as_str)
            .eq(fields.iter().copied())
    {
        return say(ctx, &m::different_fields(agent), 1);
    }
    writeln!(ctx.out, "Fields read from {agent} sessions:")?;
    for field in fields {
        writeln!(ctx.out, "  {field}")?;
    }
    // Already on: keep the date it started, or the first days would fall out of the window.
    if account.store.source_facts(source)?.enabled_since.is_none() {
        account.store.enable_source(source, now())?;
    }
    writeln!(
        ctx.out,
        "Collection on for {agent}. Reading: {}",
        folder_of(source, ctx)
    )?;
    if hint && super::super::hooks::state(ctx, source) != crate::hooks::State::Installed {
        writeln!(ctx.out, "{}", m::hook_hint(agent, source.arg()))?;
    }
    Ok(0)
}

pub fn disable(ctx: &mut Ctx, source: Source) -> Result<i32> {
    let account = open(ctx)?;
    if !account.store.is_connected()? {
        return say(ctx, m::NOT_CONNECTED, 1);
    }
    account.store.disable_source(source)?;
    say(ctx, m::DISABLED, 0)
}
