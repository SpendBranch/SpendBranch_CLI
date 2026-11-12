//! What a thread's first lines tell, and the observation a token record becomes.
use super::raw::{Payload, Record, is_id, text};
use super::{Meta, PARSER};
use crate::contract::{
    AccountRef, AgentRole, BillingMode, Branch, Count, CounterOrigin, ExternalId, Model,
    ParserVersion, PayerEvidence, Repository, Source, SourceVersion, UsageCounters,
    UsageObservation,
};
use crate::sources::repository::repository_key_of;

pub(super) fn meta_of(p: &Payload) -> Option<Meta> {
    let thread_id = text(&p.id, is_id)?.to_owned();
    let git = p.git.0.as_ref();
    Some(Meta {
        session_id: text(&p.session_id, is_id).map_or_else(|| thread_id.clone(), str::to_owned),
        subagent: text(&p.parent_thread_id, is_id).is_some(),
        cli_version: text(&p.cli_version, |v| SourceVersion::new(v).is_some()).map(str::to_owned),
        account: (p.model_provider.str() == Some("openai"))
            .then(|| text(&p.creator_account_id, |a| AccountRef::new(a).is_some()))
            .flatten()
            .map(str::to_owned),
        repository: git
            .and_then(|g| g.repository_url.str())
            .and_then(repository_key_of),
        branch: git
            .and_then(|g| text(&g.branch, |b| Branch::new(b).is_some()))
            .map(str::to_owned),
        thread_id,
    })
}

pub(super) fn observation_of(
    meta: &Meta,
    record: &Record,
    model: &str,
    plan_seen: bool,
) -> Option<UsageObservation> {
    let u = record.usage;
    let c = Count::new;
    Some(UsageObservation {
        source: Source::Codex,
        external_session_id: ExternalId::new(&meta.session_id)?,
        external_event_id: ExternalId::new(&record.response_id)?,
        occurred_at: record.at,
        model: Model::new(model)?,
        agent_role: if meta.subagent {
            AgentRole::Subagent
        } else {
            AgentRole::Main
        },
        counters: UsageCounters {
            input_tokens: c(u[0] - u[1] - u[2])?,
            output_tokens: c(u[3])?,
            cache_read_tokens: c(u[1])?,
            cache_write_tokens: c(u[2])?,
            reasoning_tokens: c(u[4])?,
            cache_write_1h_tokens: None,
            web_search_requests: None,
        },
        origin: CounterOrigin::ReportedDelta,
        parser_version: ParserVersion::new(PARSER)?,
        source_version: meta.cli_version.as_deref().and_then(SourceVersion::new),
        payer_account_ref: meta.account.as_deref().and_then(AccountRef::new),
        payer_evidence: meta.account.as_ref().map(|_| PayerEvidence::Transcript),
        billing_mode: (meta.account.is_some() && plan_seen).then_some(BillingMode::Subscription),
        repository: match meta.repository.as_deref() {
            Some(r) => Some(Repository::new(r)?),
            None => None,
        },
        branch: meta.branch.as_deref().and_then(Branch::new),
    })
}
