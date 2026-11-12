//! The account journeys: login, status, enable, disable and logout, in-process, against the
//! fake server. Nothing here waits, opens a browser or touches the real keychain.
#[path = "common/fake_server.rs"]
mod fake_server;

#[path = "common/rig.rs"]
mod rig;

use rig::{Rig, authorize};
use serde_json::{Value, json};
use spendbranch::cli::{self, messages};
use spendbranch::contract::{Iso, Source};
use spendbranch::infra::{
    keys::{KeyFiles, KeyStore, Slot},
    lock,
    store::SpoolLimits,
};
use spendbranch::sources::{claude_code, outcome::ReadOutcome};

fn observation(id: &str) -> spendbranch::contract::UsageObservation {
    serde_json::from_value(json!({
        "source": "codex", "externalSessionId": "s", "externalEventId": id,
        "occurredAt": "2026-06-01T10:00:00.000Z", "model": "m", "agentRole": "main",
        "counters": {"inputTokens": 1, "outputTokens": 1, "cacheReadTokens": 0,
                     "cacheWriteTokens": 0, "reasoningTokens": 0},
        "counterOrigin": "reported_delta", "parserVersion": "codex/v1"
    }))
    .unwrap()
}

/// One buffered item and one saved cursor for Codex.
fn buffer_something(rig: &Rig) {
    let mut outcome =
        ReadOutcome::empty(Source::Codex, Iso::from_millis(1_790_000_000_000).unwrap());
    outcome.observations.push(observation("e1"));
    outcome.cursor_set.push(("file:/x".into(), "{}".into()));
    rig.store()
        .commit_read(&outcome, SpoolLimits::default())
        .unwrap();
}

#[test]
fn login_status_and_logout_work_end_to_end() {
    let rig = Rig::new();
    authorize(&rig);

    let login = rig.run(&["login"]);
    assert_eq!(login.code, 0, "{}", login.text);
    assert!(
        login.text.contains("Organization: Acme Inc."),
        "{}",
        login.text
    );
    assert!(login.text.contains("Code: BCDF-GHJK"));
    assert!(login.text.contains(&messages::ready("Acme Inc.")));
    assert!(
        login.text.contains(messages::KEY_IN_FILE),
        "the key is in a file here"
    );
    assert_eq!(login.sleeps, [5, 5], "polled twice, a pause before each");
    assert!(login.opened.is_empty(), "no terminal, no browser");
    assert!(
        !login.text.contains("dc-abcdef"),
        "the pairing secret is never printed"
    );
    let store = rig.store();
    assert_eq!(
        store.meta_get("device_id").unwrap().as_deref(),
        Some("dev-1")
    );
    assert_eq!(store.meta_get("org_id").unwrap().as_deref(), Some("org_1"));
    assert_eq!(
        store.meta_get("org_name").unwrap().as_deref(),
        Some("Acme Inc.")
    );
    assert_eq!(
        store.meta_get("server").unwrap().as_deref(),
        Some(rig.server.base.as_str())
    );
    assert!(rig.key_file(Slot::Active) && !rig.key_file(Slot::Pending));
    assert!(rig.server.device_active("dev-1"));

    let status = rig.run(&["status"]);
    assert_eq!(status.code, 0, "{}", status.text);
    for line in [
        "Organization: Acme Inc.",
        "Member: Jane Doe",
        "Device: test-laptop (",
        "(reachable)",
        "Claude Code: active since 2026-11-01 (claude-code/v2)",
        "Local buffer: 0 items awaiting upload",
        "Claude Code: collection off here",
    ] {
        assert!(
            status.text.contains(line),
            "missing `{line}` in\n{}",
            status.text
        );
    }
    assert!(
        !status.text.contains("WARNING: this device's key"),
        "the fingerprints agree"
    );

    let logout = rig.run(&["logout"]);
    assert_eq!(logout.code, 0);
    assert!(logout.text.contains(messages::LOGOUT_REVOKED));
    assert_eq!(
        rig.server.revoked_reason("dev-1").as_deref(),
        Some("logout")
    );
    assert!(!rig.key_file(Slot::Active));
    let store = rig.store();
    assert!(!store.is_connected().unwrap());
    assert_eq!(
        store.meta_get("org_id").unwrap().as_deref(),
        Some("org_1"),
        "kept to notice a change"
    );
    assert_eq!(
        rig.run(&["status"]).text,
        format!("{}\n", messages::NOT_CONNECTED)
    );
    assert_eq!(
        rig.run(&["logout"]).text,
        format!("{}\n", messages::ALREADY_DISCONNECTED)
    );
}

#[test]
fn the_status_shows_the_fingerprint_of_the_local_key_next_to_the_servers() {
    let rig = Rig::new();
    rig.run(&["login"]);
    let key = KeyFiles::new(rig.dir.path())
        .load(Slot::Active)
        .unwrap()
        .unwrap();
    let status = rig.run(&["status"]);
    assert!(
        status
            .text
            .contains(&format!("Device: test-laptop ({})", key.fingerprint())),
        "{}",
        status.text
    );
}

#[test]
fn the_new_device_is_made_with_the_public_key_and_the_installation_id() {
    let rig = Rig::new();
    rig.run(&["login"]);
    let started = rig
        .server
        .seen()
        .into_iter()
        .find(|s| s.path == "/api/cli/v1/device-codes")
        .unwrap();
    let body: Value = serde_json::from_slice(&started.body).unwrap();
    let key = KeyFiles::new(rig.dir.path())
        .load(Slot::Active)
        .unwrap()
        .unwrap();
    assert_eq!(body["publicKey"], key.public_key());
    assert_eq!(
        body["installationId"],
        rig.store().installation_id().unwrap()
    );
    assert_eq!(body["cliVersion"], cli::VERSION);
}

#[test]
fn a_person_who_does_not_confirm_the_organization_leaves_nothing_behind() {
    for answer in ["n\n", "\n", "", "maybe\n"] {
        let rig = Rig::new();
        let out = rig.run_with(&["login"], answer, false, &[]);
        assert_eq!(out.code, 1, "{answer:?}: {}", out.text);
        assert!(out.text.contains(messages::LOGIN_CANCELLED));
        assert!(!rig.store().is_connected().unwrap());
        assert!(!rig.key_file(Slot::Active) && !rig.key_file(Slot::Pending));
        assert_eq!(
            rig.server.revoked_reason("dev-1").as_deref(),
            Some("logout"),
            "the device the server made is taken back"
        );
    }
}

#[test]
fn yes_in_any_case_confirms() {
    for answer in ["y\n", "Y\n", "yes\n", "YES\n", "  yes  \n"] {
        let rig = Rig::new();
        assert_eq!(
            rig.run_with(&["login"], answer, false, &[]).code,
            0,
            "{answer:?}"
        );
    }
}

#[test]
fn slow_down_adds_five_seconds_each_time() {
    let rig = Rig::new();
    rig.server.configure(|s| {
        s.script.slow_downs = 2;
        s.script.approve_at_poll = Some(3);
    });
    let out = rig.run(&["login"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert_eq!(out.sleeps, [5, 10, 15]);
}

#[test]
fn a_denied_or_expired_pairing_says_so_and_saves_nothing() {
    for (edit, text) in [
        (
            (|s: &mut fake_server::State| s.script.deny = true) as fn(&mut fake_server::State),
            messages::LOGIN_DENIED,
        ),
        (|s| s.script.expire = true, messages::LOGIN_EXPIRED),
    ] {
        let rig = Rig::new();
        rig.server.configure(edit);
        let out = rig.run(&["login"]);
        assert_eq!(
            (out.code, out.text.contains(text)),
            (1, true),
            "{}",
            out.text
        );
        assert!(!rig.store().is_connected().unwrap());
        assert!(!rig.key_file(Slot::Pending), "the waiting key is removed");
    }
}

#[test]
fn a_code_nobody_approves_expires_after_its_lifetime() {
    let rig = Rig::new();
    rig.server.configure(|s| s.script.approve_at_poll = None);
    let out = rig.run(&["login"]);
    assert_eq!(out.code, 1);
    assert!(out.text.contains(messages::LOGIN_EXPIRED));
    assert_eq!(out.sleeps.len(), 120, "600 s of life at 5 s a poll");
}

#[test]
fn a_verification_address_on_another_domain_stops_the_login_before_any_poll() {
    let rig = Rig::new();
    rig.server
        .configure(|s| s.script.link_origin = Some("https://evil.example".into()));
    let out = rig.run(&["login"]);
    assert_eq!(out.code, 1);
    assert!(out.text.contains(messages::LOGIN_BAD_PAIRING));
    assert!(
        !out.text.contains("evil.example"),
        "the hostile address is not shown"
    );
    assert!(!rig.paths_seen().contains(&"/api/cli/v1/tokens".to_string()));
    assert!(out.opened.is_empty());
}

#[test]
fn a_server_that_keeps_failing_ends_the_polling_after_a_few_tries() {
    let rig = Rig::new();
    rig.server.configure(|s| s.script.poll_status = Some(503));
    let out = rig.run(&["login"]);
    assert_eq!(out.code, 1);
    let polls = rig
        .paths_seen()
        .iter()
        .filter(|p| p.ends_with("/tokens"))
        .count();
    assert_eq!(polls, 4, "the first try and three more");
    assert!(!rig.store().is_connected().unwrap());
}

#[test]
fn a_server_that_cannot_be_reached_is_one_clear_message() {
    let rig = Rig::new();
    let out = rig.run_with(
        &["login"],
        "y\n",
        false,
        &[("SPENDBRANCH_SERVER", "http://127.0.0.1:1")],
    );
    assert_eq!(out.code, 1);
    assert_eq!(
        out.text,
        format!("{}\n", messages::failure_message("NETWORK"))
    );
}

#[test]
fn the_browser_opens_only_on_a_terminal_and_when_not_switched_off() {
    let link = |rig: &Rig| format!("{}/devices/pair?code=BCDF-GHJK", rig.server.base);
    let rig = Rig::new();
    assert_eq!(
        rig.run_with(&["login"], "y\n", true, &[]).opened,
        [link(&rig)]
    );
    let rig = Rig::new();
    let off = rig.run_with(&["login"], "y\n", true, &[("SPENDBRANCH_NO_BROWSER", "1")]);
    assert!(off.opened.is_empty() && off.code == 0);
    let rig = Rig::new();
    assert!(
        rig.run_with(&["login"], "y\n", false, &[])
            .opened
            .is_empty()
    );
}

#[test]
fn the_server_comes_from_the_flag_then_the_environment_and_must_be_acceptable() {
    let rig = Rig::new();
    let out = rig.run_with(
        &["login", "--server", "http://example.com"],
        "y\n",
        false,
        &[],
    );
    assert_eq!(out.code, 2);
    assert!(out.text.contains(messages::INVALID_SERVER) && out.text.contains("https://"));
    // the flag wins over the environment
    let out = rig.run_with(
        &["login", "--server", &rig.server.base],
        "y\n",
        false,
        &[("SPENDBRANCH_SERVER", "http://127.0.0.1:1")],
    );
    assert_eq!(out.code, 0, "{}", out.text);
}

#[test]
fn logging_in_again_replaces_the_device_and_keeps_the_buffer_of_the_same_organization() {
    let rig = Rig::new();
    rig.run(&["login"]);
    buffer_something(&rig);
    let again = rig.run(&["login"]);
    assert_eq!(again.code, 0, "{}", again.text);
    assert_eq!(
        rig.server.revoked_reason("dev-1").as_deref(),
        Some("replaced")
    );
    assert_eq!(
        rig.store().meta_get("device_id").unwrap().as_deref(),
        Some("dev-2")
    );
    assert!(!again.text.contains(messages::OLD_DEVICE_NOT_REVOKED));
    assert!(!again.text.contains(messages::BUFFER_DISCARDED));
    assert_eq!(
        rig.store().pending_counts().unwrap(),
        (1, 0),
        "same organization: kept"
    );
}

#[test]
fn another_organization_discards_the_buffer_compared_by_id_not_name() {
    let rig = Rig::new();
    authorize(&rig);
    rig.run(&["login"]);
    rig.run(&["enable", "codex"]);
    buffer_something(&rig);
    // a new name for the same organization is not a new organization
    rig.server
        .configure(|s| s.script.organization_name = Some("Acme Renamed".into()));
    assert!(
        !rig.run(&["login"])
            .text
            .contains(messages::BUFFER_DISCARDED)
    );
    assert_eq!(rig.store().pending_counts().unwrap(), (1, 0));
    // the send bookkeeping of the same organization stays...
    let bookkeeping = |rig: &Rig| {
        let db = rusqlite::Connection::open(rig.dir.path().join("state.db")).unwrap();
        db.execute(
            "INSERT OR REPLACE INTO rejected VALUES('CLOCK_SKEW', 3)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT OR REPLACE INTO sync_state VALUES('lastError', 'NETWORK')",
            [],
        )
        .unwrap();
    };
    bookkeeping(&rig);
    rig.run(&["login"]);
    assert_eq!(rig.store().rejected_counts().unwrap().len(), 1);
    assert!(rig.store().sync_get("lastError").unwrap().is_some());
    // another id is a new organization: its counts and errors are not ours to show
    rig.server
        .configure(|s| s.script.organization_id = Some("org_2".into()));
    let out = rig.run(&["login"]);
    assert!(
        out.text.contains(messages::BUFFER_DISCARDED),
        "{}",
        out.text
    );
    let store = rig.store();
    assert_eq!(store.pending_counts().unwrap(), (0, 0));
    assert!(store.rejected_counts().unwrap().is_empty());
    assert_eq!(store.sync_get("lastError").unwrap(), None);
    assert_eq!(store.meta_get("org_id").unwrap().as_deref(), Some("org_2"));
    assert!(
        store
            .source_facts(Source::Codex)
            .unwrap()
            .enabled_since
            .is_none(),
        "the new organization has to authorize it again"
    );
    assert_eq!(
        store.source_facts(Source::Codex).unwrap().tracked_files,
        1,
        "the cursor stays"
    );
}

#[test]
fn a_login_to_another_organization_after_a_logout_still_discards_the_buffer() {
    let rig = Rig::new();
    rig.run(&["login"]);
    buffer_something(&rig);
    rig.run(&["logout"]);
    rig.server
        .configure(|s| s.script.organization_id = Some("org_9".into()));
    let out = rig.run(&["login"]);
    assert!(
        out.text.contains(messages::BUFFER_DISCARDED),
        "{}",
        out.text
    );
    assert_eq!(rig.store().pending_counts().unwrap(), (0, 0));
}

#[test]
fn a_running_sync_holds_the_login_back_and_a_late_one_gives_up_cleanly() {
    let rig = Rig::new();
    let held = lock::try_acquire(rig.dir.path()).unwrap().unwrap();
    let out = rig.run(&["login"]);
    assert_eq!(out.code, 1);
    assert!(out.text.contains(messages::LOGIN_LOCK_BUSY));
    assert_eq!(&out.sleeps[2..], &[1u64; 59], "60 tries, a second apart");
    assert!(!rig.store().is_connected().unwrap());
    assert_eq!(
        rig.server.revoked_reason("dev-1").as_deref(),
        Some("logout")
    );
    drop(held);
    assert_eq!(
        rig.run(&["login"]).code,
        0,
        "and once it ends the login works"
    );
}

#[test]
fn status_says_what_went_wrong_and_still_shows_the_local_part() {
    let rig = Rig::new();
    rig.run(&["login"]);
    buffer_something(&rig);
    // the server revokes the device
    rig.run(&["logout"]);
    rig.server.configure(|_| {});
    // not connected any more: fresh login, then the owner revokes it behind our back
    rig.run(&["login"]);
    rig.server.configure(|s| {
        for d in s.devices.values_mut() {
            d.active = false;
        }
    });
    let out = rig.run(&["status"]);
    assert_eq!(out.code, 1);
    assert!(out.text.starts_with(&format!(
        "{}\n",
        messages::failure_message("UNAUTHENTICATED")
    )));
    assert!(
        out.text.contains("Local buffer: 1 items awaiting upload"),
        "{}",
        out.text
    );
}

#[test]
fn a_wrong_clock_is_reported_in_minutes() {
    let rig = Rig::new();
    rig.run(&["login"]);
    rig.server.configure(|s| s.clock_offset_ms = 20 * 60 * 1000);
    let out = rig.run(&["status"]);
    assert_eq!(out.code, 1);
    assert!(
        out.text.contains("minutes behind the server's"),
        "{}",
        out.text
    );
}

#[test]
fn a_missing_key_is_one_instruction_and_logout_still_cleans_up() {
    let rig = Rig::new();
    rig.run(&["login"]);
    KeyFiles::new(rig.dir.path()).delete(Slot::Active).unwrap();
    let out = rig.run(&["status"]);
    assert_eq!(out.code, 1);
    assert!(out.text.starts_with(messages::KEY_MISSING), "{}", out.text);
    let out = rig.run(&["logout"]);
    assert_eq!(out.code, 0);
    assert!(out.text.contains(messages::LOGOUT_NOT_REVOKED));
    assert!(!rig.store().is_connected().unwrap());
}

#[test]
fn logout_without_a_reachable_server_still_deletes_the_key_and_says_what_to_do() {
    let rig = Rig::new();
    rig.run(&["login"]);
    // the server moved: the saved address no longer answers
    rig.store()
        .meta_set("server", "http://127.0.0.1:1")
        .unwrap();
    let out = rig.run(&["logout"]);
    assert_eq!(out.code, 0);
    assert!(
        out.text.contains(messages::LOGOUT_NOT_REVOKED),
        "{}",
        out.text
    );
    assert!(!rig.key_file(Slot::Active));
    assert!(!rig.store().is_connected().unwrap());
}

#[test]
fn logout_of_a_device_the_server_already_revoked_says_so() {
    let rig = Rig::new();
    rig.run(&["login"]);
    rig.server.configure(|s| {
        for d in s.devices.values_mut() {
            d.active = false;
        }
    });
    let out = rig.run(&["logout"]);
    assert!(
        out.text.contains(messages::LOGOUT_ALREADY_REVOKED),
        "{}",
        out.text
    );
    assert!(!rig.key_file(Slot::Active));
}

#[test]
fn enable_checks_the_authorized_fields_exactly_and_keeps_its_start_date() {
    let rig = Rig::new();
    authorize(&rig);
    rig.run(&["login"]);
    let out = rig.run(&["enable", "claude-code"]);
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(out.text.contains("Fields read from Claude Code sessions:"));
    assert!(out.text.contains("  message.usage.input_tokens"));
    let since = rig
        .store()
        .source_facts(Source::ClaudeCode)
        .unwrap()
        .enabled_since;
    assert!(since.is_some());
    // again, with the other spelling: nothing moves
    assert_eq!(rig.run(&["enable", "claude_code"]).code, 0);
    assert_eq!(
        rig.store()
            .source_facts(Source::ClaudeCode)
            .unwrap()
            .enabled_since,
        since
    );
}

#[test]
fn enable_refuses_what_the_company_did_not_authorize_in_the_same_form() {
    let rig = Rig::new();
    authorize(&rig);
    rig.run(&["login"]);
    let agent = "Claude Code";
    let mut fields: Vec<&str> = claude_code::FIELDS.to_vec();
    let one_more: Vec<&str> = [claude_code::FIELDS.to_vec(), vec!["message.content"]].concat();
    let cases: Vec<(&str, Vec<Value>, String)> = vec![
        ("absent", vec![], messages::not_authorized(agent)),
        (
            "expired",
            vec![
                json!({"source": "claude_code", "state": "expired", "parserVersion": claude_code::PARSER,
                "fields": claude_code::FIELDS, "windowStart": null}),
            ],
            messages::not_authorized(agent),
        ),
        (
            "an unknown state is not active",
            vec![
                json!({"source": "claude_code", "state": "pending", "parserVersion": claude_code::PARSER,
                "fields": claude_code::FIELDS, "windowStart": null}),
            ],
            messages::not_authorized(agent),
        ),
        (
            "a field fewer",
            vec![
                json!({"source": "claude_code", "state": "active", "parserVersion": claude_code::PARSER,
                "fields": &claude_code::FIELDS[1..], "windowStart": null}),
            ],
            messages::different_fields(agent),
        ),
        (
            "a field more",
            vec![
                json!({"source": "claude_code", "state": "active", "parserVersion": claude_code::PARSER,
                "fields": one_more, "windowStart": null}),
            ],
            messages::different_fields(agent),
        ),
        (
            "another order",
            vec![{
                fields.swap(0, 1);
                json!({"source": "claude_code", "state": "active", "parserVersion": claude_code::PARSER,
                    "fields": fields, "windowStart": null})
            }],
            messages::different_fields(agent),
        ),
        (
            "another parser version",
            vec![
                json!({"source": "claude_code", "state": "active", "parserVersion": "claude-code/v3",
                "fields": claude_code::FIELDS, "windowStart": null}),
            ],
            messages::different_fields(agent),
        ),
    ];
    for (name, sources, text) in cases {
        rig.server.configure(|s| s.sources = sources);
        let out = rig.run(&["enable", "claude-code"]);
        assert_eq!(out.code, 1, "{name}: {}", out.text);
        assert_eq!(out.text, format!("{text}\n"), "{name}");
        assert!(
            rig.store()
                .source_facts(Source::ClaudeCode)
                .unwrap()
                .enabled_since
                .is_none(),
            "{name}: nothing was turned on"
        );
    }
}

#[test]
fn disable_turns_collection_off_and_keeps_the_reading_position() {
    let rig = Rig::new();
    authorize(&rig);
    rig.run(&["login"]);
    rig.run(&["enable", "codex"]);
    buffer_something(&rig);
    let out = rig.run(&["disable", "codex"]);
    assert_eq!(
        (out.code, out.text.as_str()),
        (0, format!("{}\n", messages::DISABLED).as_str())
    );
    let facts = rig.store().source_facts(Source::Codex).unwrap();
    assert!(facts.enabled_since.is_none());
    assert_eq!(facts.tracked_files, 1, "the cursor is still there");
    // turning it on again starts a new window, not a new reading
    assert_eq!(rig.run(&["enable", "codex"]).code, 0);
    assert_eq!(
        rig.store()
            .source_facts(Source::Codex)
            .unwrap()
            .tracked_files,
        1
    );
}

#[test]
fn status_warns_about_a_source_that_is_on_here_but_not_authorized() {
    let rig = Rig::new();
    authorize(&rig);
    rig.run(&["login"]);
    rig.run(&["enable", "claude-code"]);
    rig.server.configure(|s| s.sources.clear());
    let out = rig.run(&["status"]);
    assert!(
        out.text
            .contains("WARNING: Claude Code is on here but the company has not authorized it"),
        "{}",
        out.text
    );
}

#[test]
fn status_never_prints_text_the_server_could_use_to_attack_the_terminal() {
    let rig = Rig::new();
    rig.run(&["login"]);
    rig.server
        .configure(|s| s.organization_name = "Acme\u{1b}[2J".into());
    let out = rig.run(&["status"]);
    assert_eq!(out.code, 1);
    assert!(out.text.starts_with(messages::BAD_STATUS), "{}", out.text);
    assert!(!out.text.contains('\u{1b}'));
}

#[test]
fn status_warns_when_the_servers_fingerprint_is_not_this_devices_key() {
    let rig = Rig::new();
    rig.run(&["login"]);
    rig.server
        .configure(|s| s.script.fingerprint = Some("0000-1111-2222-3333".into()));
    let out = rig.run(&["status"]);
    assert_eq!(out.code, 0, "the call itself worked");
    assert!(
        out.text.contains("WARNING: this device's key is "),
        "{}",
        out.text
    );
    assert!(out.text.contains("0000-1111-2222-3333"));
}

#[test]
fn status_warns_in_the_last_week_of_a_device_and_not_before() {
    let rig = Rig::new();
    rig.run(&["login"]);
    let in_days = |days: u64| {
        let at = std::time::SystemTime::now() + std::time::Duration::from_secs(days * 86_400);
        Iso::from_millis(
            at.duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        )
        .unwrap()
        .to_string()
    };
    for (days, warns) in [(3, true), (6, true), (9, false), (80, false)] {
        rig.server
            .configure(|s| s.script.device_expires_at = Some(in_days(days)));
        let out = rig.run(&["status"]);
        assert_eq!(
            out.text.contains("expires soon"),
            warns,
            "{days} days left:\n{}",
            out.text
        );
    }
}

#[test]
fn a_status_answer_with_a_malformed_fingerprint_is_refused() {
    for bad in [
        "",
        "5647-5aa7-5463",
        "5647-5AA7-5463-474C",
        "5647-5aa7-5463-474g",
        "56475aa754634740000",
    ] {
        let rig = Rig::new();
        rig.run(&["login"]);
        rig.server
            .configure(|s| s.script.fingerprint = Some(bad.into()));
        let out = rig.run(&["status"]);
        assert!(
            out.text.starts_with(messages::BAD_STATUS),
            "{bad:?}: {}",
            out.text
        );
    }
}

#[test]
fn an_approval_the_browser_refused_for_lack_of_authorization_ends_as_an_expired_code_with_a_hint() {
    let rig = Rig::new();
    rig.server.configure(|s| s.script.refuse_approval = true);
    let out = rig.run(&["login"]);
    assert_eq!(out.code, 1);
    assert!(
        out.text.contains("may not have authorized collection yet"),
        "{}",
        out.text
    );
    assert_eq!(out.sleeps.len(), 120, "it kept polling until the code died");
    assert!(!rig.store().is_connected().unwrap());
}
