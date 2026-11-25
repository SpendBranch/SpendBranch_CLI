//! The pure rules of sending: batches, waiting, what each answer means.
use serde_json::json;
use spendbranch::api::{ApiFailure, ItemOutcome};
use spendbranch::contract::{CoverageReport, Iso, UsageObservation};
use spendbranch::sync::{self, Failed, Item, Unpacked};
use std::time::Duration;

fn observation(event: &str, session: &str, at: &str) -> UsageObservation {
    serde_json::from_value(json!({
        "source": "codex", "externalSessionId": session, "externalEventId": event,
        "occurredAt": at, "model": "m", "agentRole": "main",
        "counters": {"inputTokens": 1, "outputTokens": 1, "cacheReadTokens": 0,
                     "cacheWriteTokens": 0, "reasoningTokens": 0},
        "counterOrigin": "reported_delta", "parserVersion": "codex/v1"
    }))
    .unwrap()
}

fn items(n: usize) -> Vec<Item> {
    (0..n)
        .map(|i| {
            let observation = observation(&format!("e{i}"), "s", "2026-06-01T10:00:00.000Z");
            Item::new(
                i as i64 + 1,
                0,
                serde_json::to_string(&observation).unwrap(),
                observation,
            )
        })
        .collect()
}

fn failure(status: u16, code: &str, retry: Option<u64>) -> ApiFailure {
    let mut f = ApiFailure::plain(status, code);
    f.retry_after_secs = retry;
    f
}

#[test]
fn the_backoff_doubles_up_to_fifteen_minutes_with_half_to_all_of_it() {
    for (failures, base) in [
        (1, 1.0),
        (2, 2.0),
        (3, 4.0),
        (6, 32.0),
        (11, 900.0),
        (40, 900.0),
    ] {
        let (low, high) = (
            sync::backoff(failures, 0.0).as_secs_f64(),
            sync::backoff(failures, 1.0).as_secs_f64(),
        );
        assert!((low - base / 2.0).abs() < 1e-9, "{failures}: {low}");
        assert!((high - base).abs() < 1e-9, "{failures}: {high}");
    }
    assert_eq!(
        sync::backoff(0, 1.0),
        Duration::from_secs(1),
        "no failures yet is the first step"
    );
    assert_eq!(
        sync::backoff(3, 7.0),
        sync::backoff(3, 1.0),
        "the random part is clamped"
    );
}

#[test]
fn the_wait_is_the_backoff_or_what_the_server_asked_for_if_more() {
    assert_eq!(
        sync::wait_after(1, &failure(503, "UNAVAILABLE", None), 1.0),
        Duration::from_secs(1)
    );
    assert_eq!(
        sync::wait_after(1, &failure(429, "RATE_LIMITED", Some(60)), 0.0),
        Duration::from_secs(60)
    );
    assert_eq!(
        sync::wait_after(5, &failure(429, "RATE_LIMITED", Some(2)), 1.0),
        Duration::from_secs(16)
    );
}

#[test]
fn a_failure_is_passing_stopping_or_a_fault_of_the_batch_by_status_never_by_code() {
    for (f, want) in [
        (failure(0, "NETWORK", None), Failed::Transient),
        (failure(429, "RATE_LIMITED", Some(3)), Failed::Transient),
        (failure(500, "INTERNAL", None), Failed::Transient),
        (failure(502, "INTERNAL", None), Failed::Transient),
        (failure(503, "UNAVAILABLE", None), Failed::Transient),
        (failure(504, "ANYTHING_AT_ALL", None), Failed::Transient),
        (
            failure(400, "VALIDATION_FAILED", None),
            Failed::OutOfContract,
        ),
        (failure(413, "PAYLOAD_TOO_LARGE", None), Failed::Split),
        (failure(401, "UNAUTHENTICATED", None), Failed::Stop),
        (failure(401, "CLOCK_SKEW", None), Failed::Stop),
        (failure(402, "PLAN_REQUIRED", None), Failed::Stop),
        (failure(403, "FORBIDDEN", None), Failed::Stop),
        (failure(415, "UNSUPPORTED_MEDIA_TYPE", None), Failed::Stop),
        (failure(400, "SOMETHING_ELSE", None), Failed::Stop),
        (failure(200, "BAD_RESPONSE", None), Failed::Stop),
        // a 5xx is passing even when the proxy in front says a code of its own
        (failure(502, "VALIDATION_FAILED", None), Failed::Transient),
    ] {
        assert_eq!(sync::classify(&f), want, "{} {}", f.status, f.code);
    }
}

#[test]
fn a_refused_batch_is_narrowed_down_to_what_is_at_fault() {
    use sync::Narrow;
    // The coverage goes alone first, so no event is blamed for it.
    assert_eq!(sync::narrow(10, 3), Narrow::CoverageAlone);
    assert_eq!(sync::narrow(1, 1), Narrow::CoverageAlone);
    assert_eq!(sync::narrow(500, 0), Narrow::Halve(250));
    assert_eq!(sync::narrow(3, 0), Narrow::Halve(2));
    assert_eq!(sync::narrow(2, 0), Narrow::Halve(1));
    // Only what is alone can be at fault.
    assert_eq!(sync::narrow(1, 0), Narrow::DropItem);
    assert_eq!(sync::narrow(0, 4), Narrow::DropCoverage);
}

#[test]
fn server_errors_are_counted_per_batch_and_survive_the_run() {
    use sync::Strikes;
    let one = Strikes::add(None, 7, 500);
    assert_eq!(
        one,
        Strikes {
            head: 7,
            size: 500,
            count: 1
        }
    );
    let two = Strikes::add(Some(one), 7, 500);
    assert_eq!(two.count, 2);
    assert_eq!(Strikes::add(Some(two), 7, 250).count, 1, "another batch");
    assert_eq!(Strikes::add(Some(two), 8, 500).count, 1, "another head");
    assert_eq!(Strikes::parse(&two.text()), Some(two));
    for bad in ["", "1:2", "1:2:3:4", "a:2:3", "1:-2:3"] {
        assert_eq!(Strikes::parse(bad), None, "{bad}");
    }
    let narrowed = Strikes {
        head: 7,
        size: 125,
        count: 0,
    };
    assert_eq!(Strikes::limit(Some(narrowed), Some(7), 500), 125);
    assert_eq!(Strikes::limit(Some(narrowed), Some(7), 60), 60);
    assert_eq!(
        Strikes::limit(Some(narrowed), Some(9), 500),
        500,
        "another head"
    );
    assert_eq!(Strikes::limit(None, Some(7), 500), 500);
    assert_eq!(sync::SERVER_ERRORS_TO_NARROW, sync::TRIES_PER_RUN);
}

#[test]
fn the_items_the_server_keeps_failing_on_are_listed_until_it_answers_for_them() {
    let list = sync::stuck_add(None, "codex/s/e1".into(), "INTERNAL");
    let list = sync::stuck_add(list.as_deref(), "codex/s/e2".into(), "INTERNAL");
    assert_eq!(
        sync::stuck_summary(list.as_deref()),
        Some((2, "INTERNAL".into()))
    );
    let list = sync::stuck_remove(list.as_deref(), ["codex/s/e1".to_owned()]);
    assert_eq!(sync::stuck_summary(list.as_deref()).map(|s| s.0), Some(1));
    assert_eq!(
        sync::stuck_remove(list.as_deref(), ["codex/s/e2".to_owned()]),
        None
    );
    assert_eq!(sync::stuck_summary(Some("not json")), None);
    assert_eq!(sync::item_key(&items(1)[0]), "codex/s/e0");
}

#[test]
fn a_batch_is_at_most_500_observations_and_50_reports_and_never_empty() {
    let id = uuid::Uuid::new_v4();
    let report = |i: i64| {
        CoverageReport::new(
            spendbranch::contract::Source::Codex,
            Iso::from_millis(1_000 + i * 10).unwrap(),
            Iso::from_millis(2_000 + i * 10).unwrap(),
            spendbranch::contract::CoverageState::Observed,
        )
        .unwrap()
    };
    let reports: Vec<_> = (0..60).map(|i| (i, report(i))).collect();
    let p = sync::pack(id, &items(700), &reports, 500).unwrap();
    assert_eq!((p.items, p.coverage), (500, 50));
    let p = sync::pack(id, &items(700), &reports, 100).unwrap();
    assert_eq!(p.items, 100, "the caller can ask for fewer");
    let p = sync::pack(id, &[], &reports, 500).unwrap();
    assert_eq!((p.items, p.coverage), (0, 50), "reports alone are a batch");
    assert!(
        matches!(sync::pack(id, &[], &[], 500), Err(Unpacked::Nothing)),
        "nothing to send is no batch"
    );
    let v: serde_json::Value =
        serde_json::from_slice(&sync::pack(id, &items(2), &[], 500).unwrap().json).unwrap();
    assert_eq!(v["contractVersion"], "ingest/v1");
    assert_eq!(v["installationId"], id.to_string());
    assert!(
        v.get("coverage").is_none(),
        "no coverage key when there is none"
    );
}

#[test]
fn a_batch_over_the_size_is_halved_until_it_fits_and_one_that_cannot_is_refused() {
    let id = uuid::Uuid::new_v4();
    let one = sync::pack(id, &items(1), &[], 500).unwrap().json.len();
    let all = items(64);
    let p = sync::pack_within(id, &all, &[], 500, one * 10).unwrap();
    assert!(p.items > 1 && p.items <= 10, "halved to {}", p.items);
    assert!(p.json.len() <= one * 10);
    assert_eq!(p.items, 8, "64 -> 32 -> 16 -> 8");
    assert!(
        matches!(
            sync::pack_within(id, &all, &[], 500, one - 1),
            Err(Unpacked::TooBig)
        ),
        "one alone is too big"
    );
}

#[test]
fn a_batch_that_cannot_be_built_is_told_apart_from_one_that_is_too_big() {
    // Version 0 is not a standard UUID: no observation is at fault, so none may be blamed.
    let bad = uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    assert!(matches!(
        sync::pack(bad, &items(3), &[], 500),
        Err(Unpacked::Invalid)
    ));
    assert!(!sync::installation_ok(bad));
    assert!(sync::installation_ok(uuid::Uuid::new_v4()));
}

#[test]
fn rows_that_do_not_read_back_are_set_apart() {
    let good = serde_json::to_string(&observation("e1", "s", "2026-06-01T10:00:00.000Z")).unwrap();
    let (items, bad) = sync::parse_items([
        (1, good.clone(), 5),
        (2, "not json".into(), 5),
        (3, r#"{"source":"codex"}"#.into(), 5),
        (4, good, 7),
    ]);
    assert_eq!(
        items
            .iter()
            .map(|i| (i.id, i.queued_at))
            .collect::<Vec<_>>(),
        [(1, 5), (4, 7)]
    );
    assert_eq!(bad, [2, 3]);
    let (reports, bad) = sync::parse_coverage([(9, "{}".into()), (10, "x".into())]);
    assert!(reports.is_empty());
    assert_eq!(bad, [9, 10]);
}

#[test]
fn each_answer_is_applied_by_its_own_rule() {
    let now = 100 * 86_400_000;
    let mut all = items(6);
    all[1].queued_at = now - 3_600_000; // young
    all[2].queued_at = now - 25 * 3_600_000; // past the cap
    let outcomes = [
        ItemOutcome::Accepted,
        ItemOutcome::Rejected("CLOCK_SKEW".into()),
        ItemOutcome::Rejected("CLOCK_SKEW".into()),
        ItemOutcome::Duplicate,
        ItemOutcome::Rejected("FIELDS_NOT_AUTHORIZED".into()),
        ItemOutcome::Rejected("SESSION_OWNED".into()),
    ];
    let a = sync::apply_results(&all, &outcomes, now);
    assert_eq!((a.accepted, a.duplicate), (1, 1));
    assert_eq!(a.requeue, [2], "only the young CLOCK_SKEW goes back");
    assert_eq!(
        a.delete,
        [1, 3, 4, 5, 6],
        "everything else leaves, the put-back one does not"
    );
    assert_eq!(a.rejected["CLOCK_SKEW_EXPIRED"], 1);
    assert_eq!(a.rejected["FIELDS_NOT_AUTHORIZED"], 1);
    assert_eq!(a.rejected["SESSION_OWNED"], 1);
    assert!(
        !a.rejected.contains_key("CLOCK_SKEW"),
        "an item put back is not a refusal"
    );
    assert_eq!(a.session_owned, Some(("codex".into(), "s".into())));
}

#[test]
fn the_cap_is_a_day_exactly() {
    let mut one = items(1);
    let skew = [ItemOutcome::Rejected("CLOCK_SKEW".into())];
    one[0].queued_at = 0;
    assert_eq!(
        sync::apply_results(&one, &skew, sync::CLOCK_SKEW_CAP_MS).requeue,
        [1],
        "a day old is still tried"
    );
    assert!(
        sync::apply_results(&one, &skew, sync::CLOCK_SKEW_CAP_MS + 1)
            .requeue
            .is_empty()
    );
}

#[test]
fn an_item_before_the_window_is_recognized() {
    let one = &items(1)[0];
    assert!(sync::before_window(
        one,
        Iso::parse("2026-06-02T00:00:00Z").unwrap()
    ));
    assert!(
        !sync::before_window(one, Iso::parse("2026-06-01T10:00:00Z").unwrap()),
        "the start itself is in"
    );
    assert!(!sync::before_window(
        one,
        Iso::parse("2026-05-01T00:00:00Z").unwrap()
    ));
}

#[test]
fn a_refused_read_is_a_gap_from_the_last_read_or_an_hour_ago_until_now() {
    use spendbranch::contract::{CoverageState, GapReason, Source};
    let now = Iso::parse("2026-11-12T12:00:00Z").unwrap();
    let gap = |last: Option<&str>| {
        sync::full_gap(Source::Codex, last.map(|t| Iso::parse(t).unwrap()), now)
    };
    let unavailable = CoverageState::Unavailable {
        reason: GapReason::BufferFull,
    };
    for (last, from) in [
        (Some("2026-11-12T09:30:00Z"), "2026-11-12T09:30:00.000Z"),
        (None, "2026-11-12T11:00:00.000Z"),
        (Some("2026-11-12T12:00:00Z"), "2026-11-12T11:00:00.000Z"), // not before now
        (Some("2030-01-01T00:00:00Z"), "2026-11-12T11:00:00.000Z"), // from the future
    ] {
        let g = gap(last);
        assert_eq!(
            (g.from().to_string(), g.to(), g.state()),
            (from.to_string(), now, unavailable),
            "{last:?}"
        );
    }
    // at the very start of time it still makes a valid gap
    let g = sync::full_gap(Source::Codex, None, Iso::from_millis(5).unwrap());
    assert!(g.from() < g.to());
}

#[test]
fn a_saved_authorization_answer_is_reused_for_ten_minutes_and_never_from_the_future() {
    let saved = 1_000_000;
    assert!(sync::grants_fresh(saved, saved));
    assert!(sync::grants_fresh(saved, saved + sync::GRANTS_KEEP_MS - 1));
    assert!(!sync::grants_fresh(saved, saved + sync::GRANTS_KEEP_MS));
    assert!(!sync::grants_fresh(saved, saved - 1), "the clock went back");
}

#[test]
fn on_a_reused_answer_the_refusals_over_the_authorization_stay_and_are_not_counted() {
    let all = items(4);
    let outcomes = [
        ItemOutcome::Accepted,
        ItemOutcome::Rejected("COLLECTION_NOT_AUTHORIZED".into()),
        ItemOutcome::Rejected("BEFORE_COLLECTION_WINDOW".into()),
        ItemOutcome::Rejected("SESSION_OWNED".into()),
    ];
    assert!(sync::grant_refused(&outcomes));
    assert!(!sync::grant_refused(&outcomes[3..]));
    let mut applied = sync::apply_results(&all, &outcomes, 0);
    sync::spare_grant_refusals(&all, &outcomes, &mut applied);
    assert_eq!(applied.delete, [1, 4]);
    assert_eq!(
        applied.rejected.into_iter().collect::<Vec<_>>(),
        [("SESSION_OWNED".to_owned(), 1)]
    );
}
