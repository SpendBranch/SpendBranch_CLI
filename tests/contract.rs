//! The ingest/v1 types: invalid values cannot be built, and what serializes fits the schema.
use serde_json::json;
use spendbranch::contract::*;
use std::{fs, path::Path};
use uuid::Uuid;

fn schema() -> jsonschema::Validator {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/ingest-v1.json");
    jsonschema::validator_for(&serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap())
        .unwrap()
}

fn observation() -> UsageObservation {
    UsageObservation {
        source: Source::Codex,
        external_session_id: ExternalId::new("sess_1").unwrap(),
        external_event_id: ExternalId::new("resp_1").unwrap(),
        occurred_at: Iso::parse("2026-01-01T10:00:00-03:00").unwrap(),
        model: Model::new("gpt-test-1").unwrap(),
        agent_role: AgentRole::Main,
        counters: UsageCounters {
            input_tokens: Count::new(1).unwrap(),
            output_tokens: Count::UNKNOWN,
            cache_read_tokens: Count::new(0).unwrap(),
            cache_write_tokens: Count::new(0).unwrap(),
            reasoning_tokens: Count::new(0).unwrap(),
            cache_write_1h_tokens: None,
            web_search_requests: None,
        },
        origin: CounterOrigin::ReportedDelta,
        parser_version: ParserVersion::new("codex/v1").unwrap(),
        source_version: None,
        payer_account_ref: None,
        payer_evidence: None,
        billing_mode: None,
        repository: Repository::new("github.com/acme/portal"),
        branch: Branch::new("feat/x"),
    }
}

#[test]
fn text_types_enforce_their_patterns() {
    let long = |n: usize| "a".repeat(n);
    assert!(ExternalId::new("a:b_c-1").is_some());
    assert!(ExternalId::new(&long(128)).is_some());
    for bad in ["", "has space", "dot.dot", "ünï", &long(129)] {
        assert!(ExternalId::new(bad).is_none(), "{bad:?}");
    }
    assert!(Model::new("<synthetic>").is_some() && Model::new("gpt-5.1").is_some());
    assert!(Model::new("bad model!").is_none() && Model::new(&long(65)).is_none());
    assert!(SourceVersion::new("2.1.300+build-1").is_some() && SourceVersion::new("1/2").is_none());
    assert!(AccountRef::new("org_1:a.b").is_some() && AccountRef::new("a b").is_none());
    for ok in [
        "claude-code/v2",
        "codex/v1",
        "claude-code-otlp/v1",
        "a/v9999",
    ] {
        assert!(ParserVersion::new(ok).is_some(), "{ok}");
    }
    for bad in [
        "codex",
        "Codex/v1",
        "codex/v",
        "codex/v12345",
        "codex/1",
        "/v1",
    ] {
        assert!(ParserVersion::new(bad).is_none(), "{bad}");
    }
}

#[test]
fn repository_pattern() {
    for ok in [
        "github.com/acme/portal",
        "gitlab.example.com/g/sub/app",
        "host:2222/Own/Repo_1.x~",
    ] {
        assert!(Repository::new(ok).is_some(), "{ok}");
    }
    for bad in [
        "github.com/acme",
        "github.com",
        "GitHub.com/a/b",
        "host:123456/a/b",
        "host:/a/b",
        "a//b/c",
        "h/a/b c",
    ] {
        assert!(Repository::new(bad).is_none(), "{bad}");
    }
    assert!(Repository::new(&format!("h/{}/b", "a".repeat(200))).is_none());
}

#[test]
fn branch_rejects_whitespace_control_and_invisible_characters() {
    for ok in [
        "main",
        "feature/with-slash",
        "fix#123",
        "ação",
        "🚀-launch",
        &"a".repeat(255),
        &"🚀".repeat(255),
    ] {
        assert!(Branch::new(ok).is_some(), "{ok}");
    }
    for bad in [
        "",
        "has space",
        "tab\there",
        "new\nline",
        "nul\0",
        "\u{200B}zero-width",
        "bidi\u{202E}x",
        "bom\u{FEFF}",
        "soft\u{AD}hyphen",
        "nbsp\u{A0}x",
        "\u{2028}",
        &"a".repeat(256),
        &"🚀".repeat(256),
    ] {
        assert!(Branch::new(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn counters_are_capped_and_unknown_is_null() {
    assert!(Count::new(MAX_TOKENS_PER_COUNTER).is_some());
    assert!(Count::new(MAX_TOKENS_PER_COUNTER + 1).is_none());
    assert_eq!(serde_json::to_value(Count::UNKNOWN).unwrap(), json!(null));
    assert_eq!(
        serde_json::to_value(Count::new(0).unwrap()).unwrap(),
        json!(0)
    );
}

#[test]
fn timestamps_are_millisecond_utc_text() {
    let t = Iso::parse("2026-01-01T10:00:01.123456-03:00").unwrap();
    assert_eq!(t.to_string(), "2026-01-01T13:00:01.123Z");
    assert_eq!(
        Iso::from_millis(0).unwrap().to_string(),
        "1970-01-01T00:00:00.000Z"
    );
    for bad in [
        "",
        "not a date",
        "2026-01-01",
        "2026-01-01T10:00:00",
        "2026-13-01T00:00:00Z",
    ] {
        assert!(Iso::parse(bad).is_none(), "{bad}");
    }
    assert!(Iso::from_millis(-1).is_none());
}

#[test]
fn an_observation_fits_the_schema_and_keeps_optional_fields_out() {
    let value = serde_json::to_value(observation()).unwrap();
    let batch = json!({"contractVersion": "ingest/v1", "installationId": Uuid::new_v4().to_string(), "observations": [value.clone()]});
    assert!(schema().is_valid(&batch), "{value}");
    assert_eq!(value["occurredAt"], "2026-01-01T13:00:00.000Z");
    assert_eq!(value["counterOrigin"], "reported_delta");
    assert!(value.get("baseEventId").is_none() && value.get("sourceVersion").is_none());
    assert!(value["counters"].get("cacheWrite1hTokens").is_none());
}

#[test]
fn a_derived_counter_carries_its_base_event() {
    let mut o = observation();
    o.origin = CounterOrigin::DerivedFromCumulative {
        base_event_id: ExternalId::new("base_1").unwrap(),
    };
    let value = serde_json::to_value(&o).unwrap();
    assert_eq!(
        (
            value["counterOrigin"].as_str(),
            value["baseEventId"].as_str()
        ),
        (Some("derived_from_cumulative"), Some("base_1"))
    );
    let batch = json!({"contractVersion": "ingest/v1", "installationId": Uuid::new_v4().to_string(), "observations": [value]});
    assert!(schema().is_valid(&batch));
}

#[test]
fn coverage_needs_ordered_times_and_a_reason_only_when_unavailable() {
    let (a, b) = (
        Iso::from_millis(1_000).unwrap(),
        Iso::from_millis(2_000).unwrap(),
    );
    assert!(CoverageReport::new(Source::Codex, b, a, CoverageState::Observed).is_none());
    assert!(CoverageReport::new(Source::Codex, a, a, CoverageState::Observed).is_none());
    let gap = CoverageReport::new(
        Source::ClaudeCode,
        a,
        b,
        CoverageState::Unavailable {
            reason: GapReason::BufferFull,
        },
    )
    .unwrap();
    let seen = CoverageReport::new(Source::Codex, a, b, CoverageState::Observed).unwrap();
    let (g, s) = (
        serde_json::to_value(&gap).unwrap(),
        serde_json::to_value(&seen).unwrap(),
    );
    assert_eq!(
        (g["state"].as_str(), g["reason"].as_str()),
        (Some("unavailable"), Some("buffer_full"))
    );
    assert!(s.get("reason").is_none());
    let batch = json!({"contractVersion": "ingest/v1", "installationId": Uuid::new_v4().to_string(), "observations": [], "coverage": [g, s]});
    assert!(schema().is_valid(&batch));
}

#[test]
fn a_batch_respects_its_limits_and_the_schema() {
    let id = Uuid::new_v4();
    assert!(IngestBatch::new(id, vec![], vec![]).is_none(), "empty");
    assert!(
        IngestBatch::new(id, vec![observation(); MAX_BATCH_OBSERVATIONS + 1], vec![]).is_none()
    );
    let gap = CoverageReport::new(
        Source::Codex,
        Iso::from_millis(1).unwrap(),
        Iso::from_millis(2).unwrap(),
        CoverageState::Observed,
    )
    .unwrap();
    assert!(IngestBatch::new(id, vec![], vec![gap.clone(); MAX_BATCH_COVERAGE + 1]).is_none());
    assert!(
        IngestBatch::new(Uuid::from_u128(0x1234), vec![observation()], vec![]).is_none(),
        "not a standard uuid"
    );

    let full = IngestBatch::new(
        id,
        vec![observation(); MAX_BATCH_OBSERVATIONS],
        vec![gap; MAX_BATCH_COVERAGE],
    )
    .unwrap();
    let value = serde_json::to_value(&full).unwrap();
    assert!(schema().is_valid(&value));
    let only_coverage = IngestBatch::new(
        Uuid::nil(),
        vec![],
        vec![
            CoverageReport::new(
                Source::Codex,
                Iso::from_millis(1).unwrap(),
                Iso::from_millis(2).unwrap(),
                CoverageState::Observed,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    assert!(schema().is_valid(&serde_json::to_value(only_coverage).unwrap()));
}
