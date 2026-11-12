//! One usage observation, as it travels.
use super::*;
use serde::{Deserialize, Serialize};

/// `derived_from_cumulative` only exists together with the event it was derived from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "counterOrigin", rename_all = "snake_case")]
pub enum CounterOrigin {
    ReportedDelta,
    DerivedFromCumulative {
        #[serde(rename = "baseEventId")]
        base_event_id: ExternalId,
    },
}

/// Identity of an event: (`source`, `external_session_id`, `external_event_id`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "RawObservation")]
pub struct UsageObservation {
    pub source: Source,
    pub external_session_id: ExternalId,
    pub external_event_id: ExternalId,
    pub occurred_at: Iso,
    pub model: Model,
    pub agent_role: AgentRole,
    pub counters: UsageCounters,
    #[serde(flatten)]
    pub origin: CounterOrigin,
    pub parser_version: ParserVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<SourceVersion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer_account_ref: Option<AccountRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer_evidence: Option<PayerEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing_mode: Option<BillingMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<Repository>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<Branch>,
}

/// An observation as written, before the origin and its base event are checked together.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawObservation {
    source: Source,
    external_session_id: ExternalId,
    external_event_id: ExternalId,
    occurred_at: Iso,
    model: Model,
    agent_role: AgentRole,
    counters: UsageCounters,
    counter_origin: RawOrigin,
    base_event_id: Option<ExternalId>,
    parser_version: ParserVersion,
    source_version: Option<SourceVersion>,
    payer_account_ref: Option<AccountRef>,
    payer_evidence: Option<PayerEvidence>,
    billing_mode: Option<BillingMode>,
    repository: Option<Repository>,
    branch: Option<Branch>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawOrigin {
    ReportedDelta,
    DerivedFromCumulative,
}

impl TryFrom<RawObservation> for UsageObservation {
    type Error = &'static str;

    fn try_from(r: RawObservation) -> Result<Self, Self::Error> {
        let origin = match (r.counter_origin, r.base_event_id) {
            (RawOrigin::ReportedDelta, None) => CounterOrigin::ReportedDelta,
            (RawOrigin::DerivedFromCumulative, Some(base_event_id)) => {
                CounterOrigin::DerivedFromCumulative { base_event_id }
            }
            _ => return Err("baseEventId is required only with derived_from_cumulative"),
        };
        Ok(Self {
            source: r.source,
            external_session_id: r.external_session_id,
            external_event_id: r.external_event_id,
            occurred_at: r.occurred_at,
            model: r.model,
            agent_role: r.agent_role,
            counters: r.counters,
            origin,
            parser_version: r.parser_version,
            source_version: r.source_version,
            payer_account_ref: r.payer_account_ref,
            payer_evidence: r.payer_evidence,
            billing_mode: r.billing_mode,
            repository: r.repository,
            branch: r.branch,
        })
    }
}
