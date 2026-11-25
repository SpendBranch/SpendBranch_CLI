//! `sync` end to end against the fake server: what is sent, what an answer deletes, and what
//! it does when the server is away, strict or unhappy.
use crate::rig;

use rig::{Rig, authorize, now_ms, observation};
use serde_json::json;
use spendbranch::cli::messages;
use spendbranch::contract::{ExternalId, Iso};
use std::collections::VecDeque;

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

fn at_now() -> String {
    Iso::from_millis(now_ms() - HOUR).unwrap().to_string()
}

fn events(n: usize) -> Vec<spendbranch::contract::UsageObservation> {
    (0..n)
        .map(|i| observation("codex", "s1", &format!("e{i:03}"), &at_now()))
        .collect()
}

#[test]
fn a_sync_sends_the_buffer_gzipped_and_deletes_only_what_the_server_took() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(3));
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.ends_with("Uploaded 3 usage events.\n"),
        "{}",
        out.text
    );
    assert_eq!(rig.buffered(), 0);
    let s = rig.server.state.lock().unwrap();
    assert_eq!(s.stored.len(), 3);
    assert_eq!((s.batches.len(), s.batches[0].0), (1, 3));
    drop(s);
    let sent = rig
        .server
        .seen()
        .into_iter()
        .find(|r| r.path == "/api/ingest/v1/events")
        .unwrap();
    assert_eq!(sent.headers["content-encoding"], "gzip");
    assert_eq!(sent.headers["content-type"], "application/json");
    assert!(
        sent.body.starts_with(&[0x1f, 0x8b]),
        "the bytes on the wire are gzip"
    );
    assert!(rig.sync_state("lastSentAt").is_some() && rig.sync_state("lastSyncAt").is_some());
    // nothing waiting: no call at all
    let before = rig.paths_seen().len();
    let out = rig.run(&["sync"]);
    assert!(
        out.text.ends_with("Nothing new to upload.\n"),
        "{}",
        out.text
    );
    assert_eq!(
        rig.paths_seen().len(),
        before,
        "an empty buffer costs no request"
    );
}

#[test]
fn a_large_buffer_goes_in_batches_of_500() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(1200));
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    let sizes: Vec<usize> = rig
        .server
        .state
        .lock()
        .unwrap()
        .batches
        .iter()
        .map(|b| b.0)
        .collect();
    assert_eq!(sizes, [500, 500, 200]);
    assert!(out.text.contains("Uploaded 1200 usage events."));
}

#[test]
fn a_batch_the_server_finds_too_big_is_halved_until_it_fits_and_the_next_starts_full() {
    use spendbranch::sync::{self, Item};
    let rig = Rig::ready();
    // a limit that fits 15 observations but not 20, worked out from the real batch size
    let size = |n: usize| {
        let items: Vec<Item> = events(n)
            .into_iter()
            .enumerate()
            .map(|(i, observation)| Item::new(i as i64, 0, String::new(), observation))
            .collect();
        sync::pack(uuid::Uuid::new_v4(), &items, &[], 500)
            .unwrap()
            .json
            .len()
    };
    let per_item = size(2) - size(1);
    rig.server
        .configure(|s| s.ingest.max_plain = Some(size(1) + per_item * 14));
    rig.buffer(now_ms(), events(40));
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert_eq!(rig.server.state.lock().unwrap().stored.len(), 40);
    assert_eq!(rig.buffered(), 0);
    let sizes: Vec<usize> = rig
        .server
        .state
        .lock()
        .unwrap()
        .batches
        .iter()
        .map(|b| b.0)
        .collect();
    assert_eq!(
        sizes,
        [10, 15, 15],
        "40 -> 20 -> 10, halved; after an accepted batch the next starts full again: 30 -> 15"
    );
}

#[test]
fn a_source_whose_authorization_is_not_active_is_not_sent() {
    for state in ["expired", "revoked", "pending"] {
        let rig = Rig::ready();
        rig.buffer(now_ms(), events(2));
        rig.server.configure(|s| {
            for source in &mut s.sources {
                source["state"] = json!(state);
                source["windowStart"] = json!(null);
            }
        });
        let out = rig.run(&["sync"]);
        assert_eq!(out.code, 0, "{state}: {}", out.text);
        assert!(
            out.text.contains("2 Codex items stay in the buffer"),
            "{state}: {}",
            out.text
        );
        assert!(
            rig.server.state.lock().unwrap().stored.is_empty(),
            "{state}"
        );
        assert_eq!(rig.buffered(), 2, "{state}");
    }
}

#[test]
fn the_dropped_items_are_announced_on_the_first_and_every_fiftieth() {
    for (before, said) in [(0, true), (1, false), (48, false), (49, true), (50, false)] {
        let rig = Rig::ready();
        rig.buffer(now_ms(), events(1));
        rig.store()
            .sync_set("droppedItems", &before.to_string())
            .unwrap();
        rig.server.configure(|s| {
            s.ingest.fail_next = VecDeque::from(vec![(400, "VALIDATION_FAILED".into(), None)])
        });
        let out = rig.run(&["sync"]);
        assert_eq!(
            out.text.contains("item(s) so far"),
            said,
            "after {before}: {}",
            out.text
        );
        assert_eq!(
            rig.sync_state("droppedItems"),
            Some((before + 1).to_string())
        );
    }
}

#[test]
fn a_server_that_is_away_leaves_everything_in_the_buffer_and_the_next_run_sends_it_once() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(3));
    // Throttled with no time given: passing trouble that is no fault of the batch.
    rig.server.configure(|s| {
        s.ingest.fail_next = VecDeque::from(vec![
            (429, "RATE_LIMITED".into(), None),
            (429, "RATE_LIMITED".into(), None),
            (429, "RATE_LIMITED".into(), None),
        ]);
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 1, "{}", out.text);
    assert!(
        out.text
            .contains("The server requested a pause (rate limit)."),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("3 items remain buffered for the next upload."),
        "{}",
        out.text
    );
    assert_eq!(out.sleeps.len(), 2, "three tries, a pause between them");
    assert!(out.sleeps.iter().all(|s| *s <= 8));
    assert_eq!(rig.buffered(), 3);
    assert!(rig.server.state.lock().unwrap().stored.is_empty());
    let next = rig
        .sync_state("nextAttemptAt")
        .expect("the next attempt is saved");
    assert!(Iso::parse(&next).unwrap().millis() > now_ms());
    assert_eq!(rig.sync_state("failures").as_deref(), Some("3"));

    // asked again too soon, it waits without calling
    let calls = rig.paths_seen().len();
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0);
    assert!(out.text.contains("Upload deferred until"), "{}", out.text);
    assert_eq!(rig.paths_seen().len(), calls);

    // time passes; the server is back
    rig.store().sync_delete(&["nextAttemptAt"]).unwrap();
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(out.text.contains("Uploaded 3 usage events."));
    assert_eq!(rig.server.state.lock().unwrap().stored.len(), 3);
    assert_eq!(
        rig.sync_state("failures"),
        None,
        "a success forgets the trouble"
    );
    assert_eq!(rig.sync_state("nextAttemptAt"), None);
}

#[test]
fn any_answer_from_the_server_forgets_the_old_trouble() {
    // Failures from an outage long gone: the status answering is proof the server is back,
    // so one stumble on the upload waits a second, not fifteen minutes.
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(2));
    rig.store().sync_set("failures", "10").unwrap();
    rig.server.configure(|s| {
        s.ingest.fail_next = VecDeque::from(vec![(503, "UNAVAILABLE".into(), None)])
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert_eq!(out.sleeps.len(), 1, "one short pause, then the batch went");
    assert!(out.sleeps[0] <= 1);
    assert_eq!(rig.buffered(), 0);
    assert_eq!(rig.sync_state("failures"), None);
}

/// Runs `sync` as the schedule would, the waits it saved already over, until `done` or 40 runs.
fn sync_until(rig: &Rig, done: impl Fn(&Rig) -> bool) {
    for _ in 0..40 {
        if done(rig) {
            return;
        }
        rig.store().sync_delete(&["nextAttemptAt"]).unwrap();
        rig.run(&["sync"]);
    }
    assert!(done(rig), "never settled");
}

#[test]
fn a_batch_that_keeps_failing_on_the_server_is_halved_instead_of_waiting_for_ever() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(4));
    rig.server.configure(|s| {
        s.ingest.fail_next = VecDeque::from(vec![
            (503, "UNAVAILABLE".into(), None),
            (503, "UNAVAILABLE".into(), None),
            (503, "UNAVAILABLE".into(), None),
        ]);
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert_eq!(rig.buffered(), 0);
    let sizes: Vec<usize> = rig
        .server
        .state
        .lock()
        .unwrap()
        .batches
        .iter()
        .map(|b| b.0)
        .collect();
    assert_eq!(sizes, [2, 2], "a whole run of 5xx on 4, then halves");
}

#[test]
fn an_item_the_server_keeps_failing_on_stays_in_the_buffer_and_status_says_so() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(8));
    rig.server.configure(|s| {
        s.ingest
            .refuse_batch
            .insert("e003".into(), (503, "UNAVAILABLE".into()));
    });
    sync_until(&rig, |r| r.server.state.lock().unwrap().stored.len() == 7);
    let left = rig.store().queued(&["codex"], i64::MAX, 10).unwrap();
    assert_eq!(left.len(), 1, "only the item at fault waits");
    assert!(left[0].payload.contains("e003"));
    assert!(
        rig.store().rejected_counts().unwrap().is_empty(),
        "nothing was dropped"
    );
    let text = rig.run(&["status"]).text;
    assert!(
        text.contains("1 items keep failing on the server")
            && text.contains(&messages::failure_message("UNAVAILABLE")),
        "{text}"
    );
    // once the server takes it, the warning goes
    rig.server.configure(|s| s.ingest.refuse_batch.clear());
    sync_until(&rig, |r| r.buffered() == 0);
    assert!(!rig.run(&["status"]).text.contains("keep failing"));
}

#[test]
fn the_server_asking_for_a_long_pause_is_obeyed_without_waiting_for_it() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(2));
    rig.server.configure(|s| {
        s.ingest.fail_next = VecDeque::from(vec![(429, "RATE_LIMITED".into(), Some(600))])
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 1);
    assert!(out.sleeps.is_empty(), "10 minutes is not slept through");
    let next = Iso::parse(&rig.sync_state("nextAttemptAt").unwrap())
        .unwrap()
        .millis();
    assert!(
        (next - now_ms() - 600_000).abs() < 5_000,
        "next attempt in ~10 minutes"
    );
    assert_eq!(rig.buffered(), 2);
}

#[test]
fn a_next_attempt_further_out_than_any_server_may_ask_is_forgotten() {
    // Saved while the clock ran a month ahead: once it is right again, uploads must not wait a month.
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(2));
    let month_ahead = Iso::from_millis(now_ms() + 30 * DAY).unwrap().to_string();
    rig.store().sync_set("nextAttemptAt", &month_ahead).unwrap();
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert_eq!(rig.buffered(), 0);
    assert_eq!(rig.sync_state("nextAttemptAt"), None);
    // ...while one within the server's own ceiling is still obeyed.
    rig.buffer(now_ms(), events(1));
    let hours = Iso::from_millis(now_ms() + 20 * HOUR).unwrap().to_string();
    rig.store().sync_set("nextAttemptAt", &hours).unwrap();
    assert!(rig.run(&["sync"]).text.contains("Upload deferred until"));
    assert_eq!(rig.buffered(), 1);
}

#[test]
fn a_repeat_of_what_the_server_already_has_is_deleted_quietly() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(2));
    assert_eq!(rig.run(&["sync"]).code, 0);
    // a crash before the delete would have left them in the buffer: the same events again
    rig.buffer(now_ms(), events(2));
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(out.text.ends_with("No events accepted.\n"), "{}", out.text);
    assert_eq!(rig.buffered(), 0);
    assert_eq!(
        rig.server.state.lock().unwrap().stored.len(),
        2,
        "stored once"
    );
}

#[test]
fn errors_that_need_the_person_stop_the_run_and_keep_the_buffer() {
    for (status, code, text) in [
        (
            401,
            "UNAUTHENTICATED",
            messages::failure_message("UNAUTHENTICATED"),
        ),
        (
            402,
            "PLAN_REQUIRED",
            messages::failure_message("PLAN_REQUIRED"),
        ),
        (403, "FORBIDDEN", messages::failure_message("FORBIDDEN")),
        (
            415,
            "UNSUPPORTED_MEDIA_TYPE",
            "The server did not accept the upload format. A proxy in between may be changing requests; check the server address (--server).".to_owned(),
        ),
    ] {
        let rig = Rig::ready();
        rig.buffer(now_ms(), events(2));
        rig.server
            .configure(|s| s.ingest.fail_next = VecDeque::from(vec![(status, code.into(), None)]));
        let out = rig.run(&["sync"]);
        assert_eq!(out.code, 1, "{code}: {}", out.text);
        assert!(out.text.contains(&text), "{code}: {}", out.text);
        assert_eq!(rig.buffered(), 2, "{code}: the buffer stays");
        assert_eq!(rig.sync_state("lastError").as_deref(), Some(code));
        assert_eq!(
            rig.sync_state("nextAttemptAt"),
            None,
            "{code}: not a passing trouble"
        );
        assert!(out.sleeps.is_empty(), "{code}: no retry");
        // and `status` says what happened
        assert!(
            rig.run(&["status"])
                .text
                .contains(&format!("[!] Last upload failed: {text}")),
            "{code}"
        );
    }
}

#[test]
fn a_device_the_server_revoked_stops_at_the_first_call() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(2));
    rig.server.configure(|s| {
        for d in s.devices.values_mut() {
            d.active = false;
        }
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 1);
    assert!(
        out.text.contains("revoked, expired, or lost access"),
        "{}",
        out.text
    );
    assert_eq!(rig.buffered(), 2);
}

#[test]
fn one_item_the_server_calls_out_of_contract_is_found_by_halving_and_only_it_is_dropped() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(500));
    rig.server.configure(|s| {
        s.ingest
            .refuse_batch
            .insert("e321".into(), (400, "VALIDATION_FAILED".into()));
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains("Dropped 1 item(s) so far"),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("1 events dropped: the server said they were out of contract"),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("Uploaded 499 usage events."),
        "{}",
        out.text
    );
    assert_eq!(rig.buffered(), 0);
    let stored = rig.server.state.lock().unwrap().stored.clone();
    assert_eq!(stored.len(), 499, "every good item reached the server");
    assert!(stored.iter().all(|o| o["externalEventId"] != "e321"));
    assert_eq!(
        rig.store().rejected_counts().unwrap(),
        [("VALIDATION_FAILED".into(), 1)]
    );
}

#[test]
fn coverage_is_sent_alone_before_an_event_is_blamed_for_a_refused_batch() {
    let rig = Rig::ready();
    let db = rusqlite::Connection::open(rig.dir.path().join("state.db")).unwrap();
    coverage(
        &db,
        "codex",
        "2026-11-10T10:00:00.000Z",
        "2026-11-10T11:00:00.000Z",
        "observed",
        None,
    );
    rig.buffer(now_ms(), events(1));
    rig.server.configure(|s| {
        s.ingest.fail_next = VecDeque::from(vec![(400, "VALIDATION_FAILED".into(), None)])
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        rig.store().rejected_counts().unwrap().is_empty(),
        "{}",
        out.text
    );
    let s = rig.server.state.lock().unwrap();
    assert_eq!((s.stored.len(), s.stored_coverage.len()), (1, 1));
    assert_eq!(
        s.batches.iter().map(|b| (b.0, b.1)).collect::<Vec<_>>(),
        [(0, 1), (1, 0)],
        "the coverage alone, then the event"
    );
}

#[test]
fn an_installation_id_that_cannot_go_in_a_batch_stops_the_run_and_keeps_the_buffer() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(3));
    rig.store()
        .meta_set("installation_id", "00000000-0000-0000-0000-000000000001")
        .unwrap();
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 1, "{}", out.text);
    assert!(
        out.text.contains(messages::BAD_INSTALLATION_ID),
        "{}",
        out.text
    );
    assert_eq!(rig.buffered(), 3, "no event is blamed for it");
    assert!(rig.store().rejected_counts().unwrap().is_empty());
    assert!(
        !rig.paths_seen()
            .iter()
            .any(|p| p == "/api/ingest/v1/events"),
        "nothing is sent"
    );
}

#[test]
fn clock_skew_puts_an_item_back_and_after_a_day_it_is_dropped_and_counted() {
    let rig = Rig::ready();
    rig.buffer(
        now_ms() - HOUR,
        vec![observation("codex", "s1", "young", &at_now())],
    );
    rig.buffer(
        now_ms() - 25 * HOUR,
        vec![observation("codex", "s1", "old", &at_now())],
    );
    rig.buffer(
        now_ms(),
        vec![observation("codex", "s1", "fine", &at_now())],
    );
    rig.server.configure(|s| {
        s.ingest.reject.insert("young".into(), "CLOCK_SKEW".into());
        s.ingest.reject.insert("old".into(), "CLOCK_SKEW".into());
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text
            .contains("1 items dropped: their time was ahead of the server"),
        "{}",
        out.text
    );
    assert!(out.text.contains("Uploaded 1 usage events."));
    // the young one is back at the end of the queue, as old as it was
    let left = rig.store().queued(&["codex"], i64::MAX, 10).unwrap();
    assert_eq!(left.len(), 1);
    assert!(left[0].payload.contains("young"));
    assert!(
        (now_ms() - left[0].queued_at - HOUR).abs() < 60_000,
        "its age was kept"
    );
    assert_eq!(
        rig.store().rejected_counts().unwrap(),
        [("CLOCK_SKEW_EXPIRED".into(), 1)]
    );
    // and the same run does not take it again: one try per run
    assert_eq!(rig.server.state.lock().unwrap().batches.len(), 1);
}

#[test]
fn an_event_more_than_a_day_ahead_never_enters_the_buffer() {
    let rig = Rig::ready();
    let ahead = Iso::from_millis(now_ms() + 2 * DAY).unwrap().to_string();
    let near = Iso::from_millis(now_ms() + 2 * HOUR).unwrap().to_string();
    rig.buffer(
        now_ms(),
        vec![
            observation("codex", "s1", "far", &ahead),
            observation("codex", "s1", "near", &near),
        ],
    );
    assert_eq!(rig.buffered(), 1, "only the one within a day");
    assert_eq!(
        rig.store().rejected_counts().unwrap(),
        [("INVALID_LOCAL".into(), 1)]
    );
}

#[test]
fn a_session_that_belongs_to_another_member_discards_its_buffer_and_stops_the_run() {
    let rig = Rig::ready();
    let mut items = vec![observation("codex", "theirs", "t1", &at_now())];
    items.extend((0..499).map(|i| observation("codex", "mine", &format!("m{i}"), &at_now())));
    items.push(observation("codex", "theirs", "t2", &at_now())); // lands in the second batch
    items.push(observation("codex", "mine", "m-last", &at_now()));
    rig.buffer(now_ms(), items);
    rig.server.configure(|s| {
        s.ingest.reject.insert("t1".into(), "SESSION_OWNED".into());
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 1, "{}", out.text);
    assert!(
        out.text.contains(messages::SESSION_OWNED_STOP),
        "{}",
        out.text
    );
    // the other session's row that was still waiting is gone; the member's own rows are kept
    let left = rig.store().queued(&["codex"], i64::MAX, 10).unwrap();
    assert_eq!(left.len(), 1);
    assert!(left[0].payload.contains("m-last"));
    assert_eq!(rig.server.state.lock().unwrap().stored.len(), 499);
}

#[test]
fn an_answer_that_does_not_match_the_contract_keeps_the_buffer() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(3));
    rig.server.configure(|s| s.ingest.short_results = true);
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 1, "{}", out.text);
    assert_eq!(rig.buffered(), 3, "nothing is guessed");
    assert_eq!(rig.sync_state("lastError").as_deref(), Some("BAD_RESPONSE"));
}

#[test]
fn what_is_before_the_authorized_window_is_not_sent_and_is_counted() {
    let rig = Rig::new();
    authorize(&rig);
    rig.server.configure(|s| {
        for source in &mut s.sources {
            source["windowStart"] = json!("2026-11-01T00:00:00Z");
        }
    });
    assert_eq!(rig.run(&["login"]).code, 0);
    assert_eq!(rig.run(&["enable", "codex"]).code, 0);
    rig.buffer(
        now_ms(),
        vec![
            observation("codex", "s1", "old", "2026-10-15T10:00:00.000Z"),
            observation("codex", "s1", "new", &at_now()),
        ],
    );
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text
            .contains("1 events dropped: they are from before the month"),
        "{}",
        out.text
    );
    let stored = rig.server.state.lock().unwrap().stored.clone();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["externalEventId"], "new");
    assert_eq!(rig.buffered(), 0);
}

#[test]
fn a_source_the_company_has_not_authorized_stays_in_the_buffer_unsent() {
    let rig = Rig::ready();
    rig.buffer(
        now_ms(),
        vec![
            observation("codex", "s1", "c1", &at_now()),
            observation("claude_code", "s2", "a1", &at_now()),
        ],
    );
    rig.server
        .configure(|s| s.sources.retain(|x| x["source"] == "codex"));
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains("1 Claude Code items stay in the buffer"),
        "{}",
        out.text
    );
    assert_eq!(rig.server.state.lock().unwrap().stored.len(), 1);
    assert_eq!(rig.buffered(), 1);
}

#[test]
fn a_row_that_no_longer_passes_the_contract_is_removed_and_counted() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(1));
    let db = rusqlite::Connection::open(rig.dir.path().join("state.db")).unwrap();
    db.execute(
        "INSERT INTO spool(payload, queued_at) VALUES('{\"source\":\"codex\",\"nonsense\":true}', 1)",
        [],
    )
    .unwrap();
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text
            .contains("1 events dropped: they did not pass the local check"),
        "{}",
        out.text
    );
    assert_eq!(rig.buffered(), 0);
    assert_eq!(rig.server.state.lock().unwrap().stored.len(), 1);
}

#[test]
fn a_coverage_report_alone_is_sent_and_then_forgotten() {
    let rig = Rig::ready();
    let db = rusqlite::Connection::open(rig.dir.path().join("state.db")).unwrap();
    db.execute(
        "INSERT INTO pending_coverage(payload) VALUES(?1)",
        [json!({"source": "codex", "from": "2026-06-01T00:00:00.000Z",
                "to": "2026-06-01T01:00:00.000Z", "state": "unavailable", "reason": "adapter_error"})
        .to_string()],
    )
    .unwrap();
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert_eq!(rig.store().pending_counts().unwrap(), (0, 0));
    let s = rig.server.state.lock().unwrap();
    assert_eq!(s.batches, [(0, 1, s.batches[0].2)]);
    assert_eq!(s.stored_coverage.len(), 1);
}

#[test]
fn coverage_alone_that_the_server_finds_too_big_is_dropped_not_a_crash() {
    let rig = Rig::ready();
    let db = rusqlite::Connection::open(rig.dir.path().join("state.db")).unwrap();
    coverage(
        &db,
        "codex",
        "2026-06-01T00:00:00.000Z",
        "2026-06-01T01:00:00.000Z",
        "unavailable",
        Some("adapter_error"),
    );
    rig.server.configure(|s| s.ingest.max_plain = Some(10));
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert_eq!(
        rig.store().pending_counts().unwrap(),
        (0, 0),
        "a report that can never fit is not kept to fail every run"
    );
}

#[test]
fn a_batch_too_big_because_of_its_coverage_sends_the_coverage_alone_and_keeps_every_event() {
    use spendbranch::sync::{self, Item};
    let rig = Rig::ready();
    let row = |i: usize| {
        json!({"source": "claude_code", "from": format!("2026-06-01T{:02}:00:00.000Z", i % 24),
               "to": format!("2026-06-01T{:02}:30:00.000Z", i % 24), "state": "unavailable",
               "reason": "adapter_error"})
        .to_string()
    };
    let reports: Vec<_> = sync::parse_coverage((0..50).map(|i| (i as i64, row(i)))).0;
    let items: Vec<Item> = events(5)
        .into_iter()
        .enumerate()
        .map(|(i, observation)| Item::new(i as i64, 0, String::new(), observation))
        .collect();
    let size = |n: usize, c: usize| {
        let id = uuid::Uuid::new_v4();
        sync::pack(id, &items[..n], &reports[..c], 500)
            .unwrap()
            .json
            .len()
    };
    let limit = size(5, 0).max(size(0, 50));
    assert!(
        size(1, 50) > limit,
        "one event with the coverage must not fit"
    );
    rig.server.configure(|s| s.ingest.max_plain = Some(limit));
    let db = rusqlite::Connection::open(rig.dir.path().join("state.db")).unwrap();
    for i in 0..50 {
        db.execute("INSERT INTO pending_coverage(payload) VALUES(?1)", [row(i)])
            .unwrap();
    }
    rig.buffer(now_ms(), events(5));
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        !out.text.contains("did not pass"),
        "valid events blamed: {}",
        out.text
    );
    let s = rig.server.state.lock().unwrap();
    assert_eq!(s.stored.len(), 5, "every event reached the server");
    assert_eq!(s.stored_coverage.len(), 50);
}

#[test]
fn the_buffer_reaches_the_server_from_real_session_files() {
    use std::io::Write;
    let rig = Rig::ready();
    let projects = rig.dir.path().join("claude/projects/p");
    std::fs::create_dir_all(&projects).unwrap();
    let file = projects.join("s1.jsonl");
    let line = |request: &str, output: u64| {
        json!({"type": "assistant", "sessionId": "s1", "requestId": request,
               "timestamp": at_now(), "version": "2.1.300", "cwd": "/nonexistent/x",
               "message": {"id": format!("m_{request}"), "model": "claude-test-1",
                   "usage": {"input_tokens": 10, "output_tokens": output,
                             "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}})
        .to_string()
    };
    let settle = |file: &std::path::Path| {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::OpenOptions::new()
            .write(true)
            .open(file)
            .unwrap()
            .set_modified(old)
            .unwrap();
    };
    std::fs::write(&file, format!("{}\n{}\n", line("r1", 5), line("r2", 7))).unwrap();
    settle(&file);
    let dir = rig.dir.path().join("claude");
    let env = [
        ("CLAUDE_CONFIG_DIR", dir.to_str().unwrap()),
        ("CODEX_HOME", "/nonexistent/codex"),
    ];
    let out = rig.run_with(&["sync"], "", false, &env);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains("Uploaded 2 usage events."),
        "{}",
        out.text
    );
    let stored = rig.server.state.lock().unwrap().stored.clone();
    assert_eq!(stored.len(), 2);
    assert!(
        stored
            .iter()
            .all(|o| o["source"] == "claude_code" && o["parserVersion"] == "claude-code/v2")
    );
    assert!(
        !serde_json::to_string(&stored)
            .unwrap()
            .contains("nonexistent"),
        "the folder never leaves the machine"
    );
    // a second sync finds nothing new, and a new line is sent alone
    assert!(
        rig.run_with(&["sync"], "", false, &env)
            .text
            .ends_with("Nothing new to upload.\n")
    );
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap();
    writeln!(f, "{}", line("r3", 9)).unwrap();
    drop(f);
    settle(&file);
    let out = rig.run_with(&["sync"], "", false, &env);
    assert!(
        out.text.contains("Uploaded 1 usage events."),
        "{}",
        out.text
    );
    assert_eq!(rig.server.state.lock().unwrap().stored.len(), 3);
}

#[test]
fn without_a_connection_or_a_source_it_says_what_to_do() {
    let rig = Rig::new();
    assert_eq!(
        rig.run(&["sync"]).text,
        format!("{}\n", messages::NOT_CONNECTED)
    );
    authorize(&rig);
    rig.run(&["login"]);
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0);
    assert!(out.text.contains(messages::NO_SOURCE_ON), "{}", out.text);
    assert!(out.text.ends_with("Nothing new to upload.\n"));
}

#[test]
fn a_full_buffer_refuses_the_read_saves_nothing_and_records_a_gap() {
    let rig = Rig::ready();
    // One observation built through the contract, then 100 000 copies that differ in the
    // event id: going through JSON for each one took most of this test's time.
    let template = observation("codex", "s1", "e000000", &at_now());
    let items: Vec<_> = (0..100_000)
        .map(|i| {
            let mut item = template.clone();
            item.external_event_id = ExternalId::new(&format!("e{i:06}")).unwrap();
            item
        })
        .collect();
    rig.buffer(now_ms(), items);
    // the device is revoked, so the run stops after reading and the gap stays where it can be seen
    rig.server.configure(|s| {
        for d in s.devices.values_mut() {
            d.active = false;
        }
    });
    let projects = rig.dir.path().join("claude/projects/p");
    std::fs::create_dir_all(&projects).unwrap();
    let file = projects.join("s1.jsonl");
    let line = json!({"type": "assistant", "sessionId": "s1", "requestId": "r1",
        "timestamp": at_now(), "version": "2.1.300", "cwd": "/nonexistent/x",
        "message": {"id": "m1", "model": "claude-test-1",
            "usage": {"input_tokens": 10, "output_tokens": 5,
                      "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}});
    std::fs::write(&file, format!("{line}\n")).unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let dir = rig.dir.path().join("claude");
    let env = [
        ("CLAUDE_CONFIG_DIR", dir.to_str().unwrap()),
        ("CODEX_HOME", "/nonexistent/codex"),
    ];
    let out = rig.run_with(&["sync"], "", false, &env);
    assert!(
        out.text.contains("Claude Code: the local buffer is full"),
        "{}",
        out.text
    );
    let store = rig.store();
    assert_eq!(
        store.pending_counts().unwrap(),
        (100_000, 1),
        "nothing was added but the gap"
    );
    assert_eq!(
        store
            .source_facts(spendbranch::contract::Source::ClaudeCode)
            .unwrap()
            .tracked_files,
        0,
        "the file will be read again"
    );
    let db = rusqlite::Connection::open(rig.dir.path().join("state.db")).unwrap();
    let gap: String = db
        .query_row("SELECT payload FROM pending_coverage", [], |r| r.get(0))
        .unwrap();
    let gap: serde_json::Value = serde_json::from_str(&gap).unwrap();
    assert_eq!(
        (
            gap["source"].as_str(),
            gap["state"].as_str(),
            gap["reason"].as_str()
        ),
        (
            Some("claude_code"),
            Some("unavailable"),
            Some("buffer_full")
        )
    );
}

#[test]
fn coverage_of_a_source_that_is_not_active_is_kept_here_and_not_sent() {
    let rig = Rig::ready();
    rig.server
        .configure(|s| s.sources.retain(|x| x["source"] == "codex"));
    let db = rusqlite::Connection::open(rig.dir.path().join("state.db")).unwrap();
    for source in ["codex", "claude_code"] {
        db.execute(
            "INSERT INTO pending_coverage(payload) VALUES(?1)",
            [json!({"source": source, "from": "2026-06-01T00:00:00.000Z",
                    "to": "2026-06-01T01:00:00.000Z", "state": "observed"})
            .to_string()],
        )
        .unwrap();
    }
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    let s = rig.server.state.lock().unwrap();
    assert_eq!(s.stored_coverage.len(), 1);
    assert_eq!(s.stored_coverage[0]["source"], "codex");
    drop(s);
    assert_eq!(
        rig.store().pending_counts().unwrap(),
        (0, 1),
        "the other waits for its authorization"
    );
}

#[test]
fn coverage_that_ends_past_the_server_clock_window_is_sent_and_ignored_like_the_api_does() {
    let rig = Rig::ready();
    let db = rusqlite::Connection::open(rig.dir.path().join("state.db")).unwrap();
    coverage(
        &db,
        "claude_code",
        "2026-06-01T00:00:00.000Z",
        "2099-01-01T00:00:00.000Z",
        "observed",
        None,
    );
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(rig.server.state.lock().unwrap().stored_coverage.is_empty());
    assert_eq!(
        rig.store().pending_counts().unwrap(),
        (0, 0),
        "the server answered 200, so nothing waits here"
    );
}

fn coverage(
    db: &rusqlite::Connection,
    source: &str,
    from: &str,
    to: &str,
    state: &str,
    reason: Option<&str>,
) {
    let mut v = json!({"source": source, "from": from, "to": to, "state": state});
    if let Some(r) = reason {
        v["reason"] = json!(r);
    }
    db.execute(
        "INSERT INTO pending_coverage(payload) VALUES(?1)",
        [v.to_string()],
    )
    .unwrap();
}

#[test]
fn the_status_shows_what_the_server_has_seen_and_the_gaps_still_open() {
    let rig = Rig::ready();
    // nothing sent yet: the list is empty
    assert!(!rig.run(&["status"]).text.contains("last contact"));
    rig.buffer(now_ms(), events(2));
    let db = rusqlite::Connection::open(rig.dir.path().join("state.db")).unwrap();
    coverage(
        &db,
        "codex",
        "2026-11-10T10:00:00.000Z",
        "2026-11-10T11:00:00.000Z",
        "unavailable",
        Some("adapter_error"),
    );
    coverage(
        &db,
        "codex",
        "2026-11-11T10:00:00.000Z",
        "2026-11-11T11:00:00.000Z",
        "unavailable",
        Some("buffer_full"),
    );
    coverage(
        &db,
        "codex",
        "2026-11-12T08:00:00.000Z",
        "2026-11-12T09:00:00.000Z",
        "observed",
        None,
    );
    assert_eq!(rig.run(&["sync"]).code, 0);
    let text = rig.run(&["status"]).text;
    assert!(
        text.contains("last contact less than an hour ago")
            && text.contains("observed until 2026-11-12T09:00:00"),
        "{text}"
    );
    assert!(
        text.contains("[!] Codex: open gap 2026-11-11T10:00:00")
            && text.contains("(buffer_full): this is not zero spending."),
        "{text}"
    );
    assert!(
        text.contains("open gap 2026-11-10T10:00:00") && text.contains("(adapter_error)"),
        "{text}"
    );
    assert!(
        text.find("2026-11-11T10").unwrap() < text.find("2026-11-10T10").unwrap(),
        "the gap that starts latest comes first"
    );
    // an `observed` told later that covers a gap whole closes it; one that covers part does not
    coverage(
        &db,
        "codex",
        "2026-11-11T09:00:00.000Z",
        "2026-11-11T12:00:00.000Z",
        "observed",
        None,
    );
    coverage(
        &db,
        "codex",
        "2026-11-10T10:00:00.000Z",
        "2026-11-10T10:30:00.000Z",
        "observed",
        None,
    );
    assert_eq!(rig.run(&["sync"]).code, 0);
    let text = rig.run(&["status"]).text;
    assert!(!text.contains("(buffer_full)"), "closed: {text}");
    assert!(
        text.contains("(adapter_error)"),
        "half covered stays open: {text}"
    );
    // an older `observed` does not take the furthest time back
    coverage(
        &db,
        "codex",
        "2026-11-01T00:00:00.000Z",
        "2026-11-02T00:00:00.000Z",
        "observed",
        None,
    );
    assert_eq!(rig.run(&["sync"]).code, 0);
    assert!(
        rig.run(&["status"])
            .text
            .contains("observed until 2026-11-12T09:00:00")
    );
}

fn status_calls(rig: &Rig) -> usize {
    rig.paths_seen()
        .iter()
        .filter(|p| *p == "/api/cli/v1/status")
        .count()
}

#[test]
fn what_the_company_authorized_is_asked_once_per_ten_minutes_and_status_always_asks() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(1));
    let before = status_calls(&rig);
    assert_eq!(rig.run(&["sync"]).code, 0);
    assert_eq!(status_calls(&rig), before + 1);
    rig.buffer(
        now_ms(),
        vec![observation("codex", "s1", "later", &at_now())],
    );
    let out = rig.run(&["sync"]);
    assert!(
        out.text.contains("Uploaded 1 usage events."),
        "{}",
        out.text
    );
    assert_eq!(
        status_calls(&rig),
        before + 1,
        "the second sync reuses the answer"
    );
    assert_eq!(rig.run(&["status"]).code, 0);
    assert_eq!(status_calls(&rig), before + 2, "`status` always asks");
    // ten minutes later it is asked again
    let saved: serde_json::Value =
        serde_json::from_str(&rig.sync_state("grantsView").unwrap()).unwrap();
    let mut old = saved.clone();
    old["at"] = json!(saved["at"].as_i64().unwrap() - 10 * 60_000);
    rig.store()
        .sync_set("grantsView", &old.to_string())
        .unwrap();
    rig.buffer(
        now_ms(),
        vec![observation("codex", "s1", "much-later", &at_now())],
    );
    assert_eq!(rig.run(&["sync"]).code, 0);
    assert_eq!(status_calls(&rig), before + 3);
}

#[test]
fn a_refusal_over_the_authorization_asks_again_and_keeps_what_a_reused_answer_sent() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(1));
    assert_eq!(rig.run(&["sync"]).code, 0);
    // The company turns Codex off; the answer this device keeps still says it is on.
    rig.server.configure(|s| {
        s.sources.retain(|x| x["source"] != "codex");
        s.ingest
            .reject
            .insert("late".into(), "COLLECTION_NOT_AUTHORIZED".into());
    });
    rig.buffer(
        now_ms(),
        vec![observation("codex", "s1", "late", &at_now())],
    );
    let before = status_calls(&rig);
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert_eq!(
        status_calls(&rig),
        before + 1,
        "asked again after the refusal"
    );
    assert!(
        out.text.contains("1 Codex items stay in the buffer"),
        "{}",
        out.text
    );
    assert_eq!(rig.buffered(), 1, "kept: the answer it went out on was old");
    assert!(rig.store().rejected_counts().unwrap().is_empty());
}

#[test]
fn a_refusal_on_a_fresh_answer_counts_and_the_next_sync_asks_again() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), vec![observation("codex", "s1", "odd", &at_now())]);
    rig.server.configure(|s| {
        s.ingest
            .reject
            .insert("odd".into(), "FIELDS_NOT_AUTHORIZED".into());
    });
    assert_eq!(rig.run(&["sync"]).code, 0);
    assert_eq!(rig.buffered(), 0);
    assert_eq!(
        rig.store().rejected_counts().unwrap(),
        [("FIELDS_NOT_AUTHORIZED".into(), 1)]
    );
    assert_eq!(
        rig.sync_state("grantsView"),
        None,
        "the answer is forgotten"
    );
    rig.buffer(now_ms(), events(1));
    let before = status_calls(&rig);
    assert_eq!(rig.run(&["sync"]).code, 0);
    assert_eq!(status_calls(&rig), before + 1);
}

#[test]
fn a_reused_answer_never_drops_an_item_as_before_the_window() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(1));
    assert_eq!(rig.run(&["sync"]).code, 0);
    // The window moved back since: the old answer would call this item too early.
    rig.server.configure(|s| {
        for source in &mut s.sources {
            source["windowStart"] = json!("2026-10-01T00:00:00Z");
        }
    });
    rig.buffer(
        now_ms(),
        vec![observation(
            "codex",
            "s1",
            "october",
            "2026-10-15T10:00:00.000Z",
        )],
    );
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains("Uploaded 1 usage events."),
        "{}",
        out.text
    );
    assert!(rig.store().rejected_counts().unwrap().is_empty());
}

#[test]
fn turning_a_source_on_or_off_or_signing_in_or_out_forgets_the_answer() {
    let rig = Rig::ready();
    let kept = |rig: &Rig| rig.sync_state("grantsView").is_some();
    let sync = |rig: &Rig| {
        rig.buffer(
            now_ms(),
            vec![observation(
                "codex",
                "s1",
                &format!("e{}", now_ms()),
                &at_now(),
            )],
        );
        assert_eq!(rig.run(&["sync"]).code, 0);
        assert!(kept(rig));
    };
    for step in [
        &["disable", "codex"][..],
        &["enable", "codex"][..],
        &["login"][..],
        &["logout"][..],
    ] {
        sync(&rig);
        rig.run(step);
        assert!(!kept(&rig), "{step:?}");
    }
}
