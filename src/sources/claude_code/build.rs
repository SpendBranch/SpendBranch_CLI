//! What a request's counters and what it was become: the observation that is sent.
use super::PARSER;
use super::raw::{Counters, Pending, Usage};
use crate::contract::{
    AccountRef, Branch, Count, CounterOrigin, ExternalId, Iso, Model, ParserVersion, PayerEvidence,
    Source, SourceVersion, UsageCounters, UsageObservation,
};
use crate::sources::json::{Counter, Leaf, counter};

pub(super) fn counters_of(u: &Usage) -> Option<Counters> {
    let details = u
        .output_tokens_details
        .0
        .as_ref()
        .map(|d| &d.thinking_tokens);
    let cache = u
        .cache_creation
        .0
        .as_ref()
        .map(|c| &c.ephemeral_1h_input_tokens);
    let tools = u.server_tool_use.0.as_ref().map(|t| &t.web_search_requests);
    let missing = Leaf::Missing;
    let leaves = [
        &u.input_tokens,
        &u.output_tokens,
        &u.cache_read_input_tokens,
        &u.cache_creation_input_tokens,
        details.unwrap_or(&missing),
        cache.unwrap_or(&missing),
        tools.unwrap_or(&missing),
    ];
    let mut out = [None; 7];
    for (slot, l) in out.iter_mut().zip(leaves) {
        *slot = match counter(l) {
            Counter::Unknown => None,
            Counter::Value(n) => Some(n),
            Counter::Bad => return None,
        };
    }
    Some(out)
}

pub(super) fn observation_of(
    key: &str,
    m: &Pending,
    payer: Option<&str>,
) -> Option<UsageObservation> {
    let count = |c: Option<u64>| c.map_or(Some(Count::UNKNOWN), Count::new);
    let [
        input,
        output,
        cache_read,
        cache_write,
        reasoning,
        cache_write_1h,
        web,
    ] = m.counters;
    Some(UsageObservation {
        source: Source::ClaudeCode,
        external_session_id: ExternalId::new(&m.session_id)?,
        external_event_id: ExternalId::new(key)?,
        occurred_at: Iso::from_millis(m.at)?,
        model: Model::new(&m.model)?,
        agent_role: m.role,
        counters: UsageCounters {
            input_tokens: count(input)?,
            output_tokens: count(output)?,
            cache_read_tokens: count(cache_read)?,
            cache_write_tokens: count(cache_write)?,
            reasoning_tokens: count(reasoning)?,
            cache_write_1h_tokens: Some(count(cache_write_1h)?),
            web_search_requests: Some(count(web)?),
        },
        origin: CounterOrigin::ReportedDelta,
        parser_version: ParserVersion::new(PARSER)?,
        source_version: m.version.as_deref().and_then(SourceVersion::new),
        payer_account_ref: payer.and_then(AccountRef::new),
        payer_evidence: payer.map(|_| PayerEvidence::Transcript),
        billing_mode: None,
        repository: None,
        branch: m.branch.as_deref().and_then(Branch::new),
    })
}
