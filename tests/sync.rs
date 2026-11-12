//! `sync` end to end against the fake server: what is sent, what an answer deletes, and what
//! it does when the server is away, strict or unhappy.
#[path = "common/fake_server.rs"]
mod fake_server;
#[path = "common/rig.rs"]
mod rig;

use rig::{Rig, authorize, now_ms, observation};
use serde_json::json;
use spendbranch::cli::messages;
use spendbranch::contract::Iso;
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
fn a_batch_the_server_finds_too_big_is_halved_until_it_fits() {
    use spendbranch::sync::{self, Item};
    let rig = Rig::ready();
    // a limit that fits 15 observations but not 20, worked out from the real batch size
    let size = |n: usize| {
        let items: Vec<Item> = events(n)
            .into_iter()
            .enumerate()
            .map(|(i, observation)| Item {
                id: i as i64,
                queued_at: 0,
                payload: String::new(),
                observation,
            })
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
        [10, 10, 10, 10],
        "40 -> 20 -> 10: halved, not trimmed one at a time"
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
fn the_dropped_batches_are_announced_on_the_first_and_every_fiftieth() {
    for (before, said) in [(0, true), (1, false), (48, false), (49, true), (50, false)] {
        let rig = Rig::ready();
        rig.buffer(now_ms(), events(2));
        rig.store()
            .sync_set("droppedBatches", &before.to_string())
            .unwrap();
        rig.server.configure(|s| {
            s.ingest.fail_next = VecDeque::from(vec![(400, "VALIDATION_FAILED".into(), None)])
        });
        let out = rig.run(&["sync"]);
        assert_eq!(
            out.text.contains("batch(es) so far"),
            said,
            "after {before}: {}",
            out.text
        );
        assert_eq!(
            rig.sync_state("droppedBatches"),
            Some((before + 1).to_string())
        );
    }
}

#[test]
fn a_server_that_is_away_leaves_everything_in_the_buffer_and_the_next_run_sends_it_once() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(3));
    rig.server.configure(|s| {
        s.ingest.fail_next = VecDeque::from(vec![
            (503, "UNAVAILABLE".into(), None),
            (503, "UNAVAILABLE".into(), None),
            (503, "UNAVAILABLE".into(), None),
        ]);
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 1, "{}", out.text);
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
            "The server rejected the request (UNSUPPORTED_MEDIA_TYPE).".to_owned(),
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
                .contains(&format!("Last error: {text}")),
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
    assert!(out.text.contains("revoked or expired"), "{}", out.text);
    assert_eq!(rig.buffered(), 2);
}

#[test]
fn a_batch_the_server_calls_out_of_contract_is_dropped_and_counted_and_the_rest_goes() {
    let rig = Rig::ready();
    rig.buffer(now_ms(), events(600));
    rig.server.configure(|s| {
        s.ingest.fail_next = VecDeque::from(vec![(400, "VALIDATION_FAILED".into(), None)])
    });
    let out = rig.run(&["sync"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains("Dropped 1 batch(es) so far"),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("500 events dropped: the server said the batch was out of contract")
    );
    assert!(out.text.contains("Uploaded 100 usage events."));
    assert_eq!(rig.buffered(), 0);
    assert_eq!(
        rig.store().rejected_counts().unwrap(),
        [("VALIDATION_FAILED".into(), 500)]
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
    let items: Vec<_> = (0..100_000)
        .map(|i| observation("codex", "s1", &format!("e{i:06}"), &at_now()))
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
