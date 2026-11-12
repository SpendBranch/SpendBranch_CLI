//! The rules for the agents' hook files and for the hook itself: what an edit keeps, what it
//! replaces, and what never gets into the log.
use serde_json::{Value, json};
use spendbranch::contract::{Iso, Source};
use spendbranch::hooks::{
    self, Plan, Problem, State, command, install, is_ours, last_firing, log_line, parse_input,
    plain, state, uninstall,
};

const EXE: &str = "/home/me/.local/bin/spendbranch";
const CLAUDE: Source = Source::ClaudeCode;
const CODEX: Source = Source::Codex;

fn written(plan: Result<Plan, Problem>) -> String {
    match plan.unwrap() {
        Plan::Write(text) => text,
        Plan::Unchanged => panic!("expected a change"),
    }
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn installed(source: Source, original: Option<&str>) -> String {
    written(install(source, original, EXE, false))
}

#[test]
fn claude_gets_session_end_and_stop_in_the_documented_shape() {
    let text = installed(CLAUDE, None);
    let line = format!("'{EXE}' hook claude-code");
    let entry = json!({"type": "command", "command": line, "async": true, "timeout": 10});
    assert_eq!(
        json_of(&text),
        json!({"hooks": {
            "SessionEnd": [{"hooks": [entry.clone()]}],
            "Stop": [{"hooks": [entry]}],
        }})
    );
    assert!(
        text.ends_with("}\n"),
        "a final newline, like the agents write"
    );
}

#[test]
fn codex_session_end_is_synchronous_and_short_and_stop_is_background() {
    let value = json_of(&installed(CODEX, None));
    assert_eq!(
        value["hooks"]["SessionEnd"][0]["hooks"][0],
        json!({"type": "command", "command": format!("'{EXE}' hook codex SessionEnd"), "timeout": 3})
    );
    assert_eq!(
        value["hooks"]["Stop"][0]["hooks"][0],
        json!({"type": "command", "command": format!("'{EXE}' hook codex Stop"), "async": true, "timeout": 30})
    );
}

#[test]
fn installing_twice_changes_nothing() {
    for source in [CLAUDE, CODEX] {
        let once = installed(source, None);
        assert_eq!(
            install(source, Some(&once), EXE, false),
            Ok(Plan::Unchanged),
            "{source:?}"
        );
        assert_eq!(state(source, Some(&once), EXE, false), State::Installed);
    }
}

#[test]
fn everything_that_is_not_ours_stays_and_the_key_order_too() {
    let original = r#"{
  "zeta": 1,
  "permissions": {
    "allow": [
      "Bash(ls)"
    ]
  },
  "hooks": {
    "Stop": [
      {
        "matcher": "x",
        "hooks": [
          {
            "type": "command",
            "command": "echo mine"
          }
        ]
      }
    ],
    "PreToolUse": []
  },
  "alpha": 2
}
"#;
    let text = installed(CLAUDE, Some(original));
    let value = json_of(&text);
    assert_eq!(
        value["hooks"]["Stop"][0]["hooks"][0]["command"],
        "echo mine"
    );
    assert_eq!(value["hooks"]["Stop"][0]["matcher"], "x");
    assert_eq!(
        value["hooks"]["Stop"].as_array().unwrap().len(),
        2,
        "ours is a new group"
    );
    assert_eq!(value["hooks"]["PreToolUse"], json!([]));
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["zeta", "permissions", "hooks", "alpha"]);
    // And taking ours out gives back the very same bytes.
    assert_eq!(written(uninstall(CLAUDE, Some(&text))), original);
}

#[test]
fn uninstall_after_install_gives_back_the_original_file() {
    let original = "{\n  \"model\": \"opus\"\n}\n";
    for source in [CLAUDE, CODEX] {
        let after = installed(source, Some(original));
        assert_ne!(after, original);
        assert_eq!(
            written(uninstall(source, Some(&after))),
            original,
            "{source:?}"
        );
    }
}

#[test]
fn uninstall_takes_only_our_entry_out_of_a_group_it_shares() {
    let original = json!({"hooks": {"Stop": [{"hooks": [
        {"type": "command", "command": "echo mine"},
        {"type": "command", "command": format!("'{EXE}' hook claude-code"), "async": true},
    ]}], "SessionEnd": [{"hooks": [
        {"type": "command", "command": format!("'{EXE}' hook claude-code")},
    ]}]}})
    .to_string();
    let value = json_of(&written(uninstall(CLAUDE, Some(&original))));
    assert_eq!(
        value,
        json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo mine"}]}]}})
    );
}

#[test]
fn uninstall_removes_the_hooks_key_only_when_it_is_left_empty_by_us() {
    let ours = installed(CLAUDE, None);
    assert_eq!(json_of(&written(uninstall(CLAUDE, Some(&ours)))), json!({}));
    // An empty `hooks` the person wrote is theirs.
    assert_eq!(
        uninstall(CLAUDE, Some(r#"{"hooks": {}}"#)),
        Ok(Plan::Unchanged)
    );
    assert_eq!(
        uninstall(CLAUDE, None),
        Ok(Plan::Unchanged),
        "no file, nothing to do"
    );
}

#[test]
fn the_line_the_typescript_cli_wrote_is_replaced_not_duplicated() {
    let old = "node '/home/me/.npm/spendbranch/dist/cli.js' hook claude-code";
    let original = json!({"hooks": {
        "SessionEnd": [{"hooks": [{"type": "command", "command": old, "async": true, "timeout": 10}]}],
    }})
    .to_string();
    assert_eq!(state(CLAUDE, Some(&original), EXE, false), State::Outdated);
    let value = json_of(&installed(CLAUDE, Some(&original)));
    let session_end = value["hooks"]["SessionEnd"].as_array().unwrap();
    assert_eq!(session_end.len(), 1);
    assert_eq!(
        session_end[0]["hooks"][0]["command"],
        format!("'{EXE}' hook claude-code")
    );
    assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);

    let old_codex = "node '/x/spendbranch/dist/cli.js' hook codex Stop";
    let original =
        json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": old_codex}]}]}})
            .to_string();
    let value = json_of(&installed(CODEX, Some(&original)));
    assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["hooks"]["Stop"][0]["hooks"][0]["command"],
        format!("'{EXE}' hook codex Stop")
    );
}

#[test]
fn a_moved_binary_is_an_outdated_hook_and_installing_fixes_it() {
    let text = installed(CLAUDE, None);
    assert_eq!(
        state(CLAUDE, Some(&text), "/elsewhere/spendbranch", false),
        State::Outdated
    );
    let fixed = written(install(
        CLAUDE,
        Some(&text),
        "/elsewhere/spendbranch",
        false,
    ));
    assert_eq!(
        state(CLAUDE, Some(&fixed), "/elsewhere/spendbranch", false),
        State::Installed
    );
    assert!(!fixed.contains(EXE), "the old path is gone");
}

#[test]
fn duplicates_of_ours_collapse_to_one() {
    let line = format!("'{EXE}' hook claude-code");
    let entry = json!({"type": "command", "command": line});
    let original = json!({"hooks": {"Stop": [
        {"hooks": [entry.clone()]}, {"hooks": [entry.clone(), entry]},
    ]}})
    .to_string();
    let value = json_of(&installed(CLAUDE, Some(&original)));
    let stop = value["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 1, "the groups we emptied are gone");
    assert_eq!(stop[0]["hooks"].as_array().unwrap().len(), 1);
}

#[test]
fn state_tells_the_four_cases_apart() {
    let one = json!({"hooks": {"Stop": [{"hooks": [
        {"type": "command", "command": format!("'{EXE}' hook claude-code")},
    ]}]}})
    .to_string();
    assert_eq!(state(CLAUDE, None, EXE, false), State::NotInstalled);
    assert_eq!(state(CLAUDE, Some("{}"), EXE, false), State::NotInstalled);
    assert_eq!(
        state(CLAUDE, Some(&one), EXE, false),
        State::Outdated,
        "SessionEnd is missing"
    );
    assert_eq!(state(CLAUDE, Some("not json"), EXE, false), State::Invalid);
    assert_eq!(
        state(CLAUDE, Some(r#"{"hooks": 3}"#), EXE, false),
        State::Invalid
    );
}

#[test]
fn a_file_that_is_not_an_object_or_has_another_layout_is_refused_untouched() {
    for text in ["", "not json", "[]", "3", "null", "{\"a\": ", "// c\n{}"] {
        assert_eq!(
            install(CLAUDE, Some(text), EXE, false),
            Err(Problem::NotJson),
            "{text:?}"
        );
        assert_eq!(
            uninstall(CLAUDE, Some(text)),
            Err(Problem::NotJson),
            "{text:?}"
        );
    }
    for text in [
        r#"{"hooks": []}"#,
        r#"{"hooks": {"Stop": {}}}"#,
        r#"{"hooks": {"SessionEnd": 1}}"#,
    ] {
        assert_eq!(
            install(CLAUDE, Some(text), EXE, false),
            Err(Problem::Layout),
            "{text}"
        );
    }
}

#[test]
fn only_our_command_lines_are_recognised() {
    assert!(is_ours(CLAUDE, "'/x/spendbranch' hook claude-code"));
    assert!(is_ours(
        CLAUDE,
        "node '/x/spendbranch/cli.js' hook claude-code  "
    ));
    assert!(!is_ours(CLAUDE, "echo spendbranch"));
    assert!(!is_ours(CLAUDE, "'/x/other' hook claude-code"));
    assert!(!is_ours(CLAUDE, "'/x/spendbranch' hook codex Stop"));
    assert!(is_ours(CODEX, "'/x/spendbranch' hook codex Stop"));
    assert!(!is_ours(CODEX, "'/x/spendbranch' hook claude-code"));
}

#[test]
fn the_command_is_quoted_for_the_shell_of_each_system() {
    assert_eq!(
        command("/home/o'neil/bin/spendbranch", false, "hook codex Stop"),
        "'/home/o'\\''neil/bin/spendbranch' hook codex Stop"
    );
    assert_eq!(
        command(
            "C:\\Users\\me\\AppData\\spendbranch.exe",
            true,
            "hook claude-code"
        ),
        "\"C:/Users/me/AppData/spendbranch.exe\" hook claude-code"
    );
}

// --- the hook itself ------------------------------------------------------------------

#[test]
fn what_the_agent_sends_is_reduced_to_the_event_and_the_session() {
    let input = json!({
        "hook_event_name": "Stop", "session_id": "abc-123",
        "transcript_path": "/home/me/secret.jsonl", "prompt": "my password is hunter2",
    })
    .to_string();
    assert_eq!(
        parse_input(input.as_bytes()),
        (Some("Stop".into()), Some("abc-123".into()))
    );
    // Odd characters, too long, wrong type, not JSON: nothing gets through.
    let long = "a".repeat(65);
    for body in [
        json!({"hook_event_name": "Stop; rm -rf", "session_id": long}).to_string(),
        json!({"hook_event_name": 5, "session_id": ""}).to_string(),
        "garbage".into(),
        String::new(),
    ] {
        assert_eq!(parse_input(body.as_bytes()), (None, None), "{body}");
    }
    assert_eq!(plain("SessionEnd").as_deref(), Some("SessionEnd"));
    assert_eq!(plain("a b"), None);
}

#[test]
fn the_log_line_carries_the_time_the_event_and_the_session_and_nothing_else() {
    let at = Iso::parse("2026-11-12T10:00:00Z").unwrap();
    let line = log_line(at, "Stop", Some("s1"));
    assert_eq!(
        json_of(&line),
        json!({"at": "2026-11-12T10:00:00.000Z", "event": "Stop", "session": "s1"})
    );
    assert_eq!(
        json_of(&log_line(at, "codex:Stop", None))
            .as_object()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn the_last_firing_is_the_newest_line_of_that_agent() {
    let at = |s| Iso::parse(s).unwrap();
    let log = [
        log_line(at("2026-11-12T08:00:00Z"), "codex:Stop", None),
        log_line(at("2026-11-12T09:00:00Z"), "Stop", Some("x")),
        "not a json line".into(),
        log_line(at("2026-11-12T10:00:00Z"), "codex:SessionEnd", None),
        log_line(at("2026-11-12T11:00:00Z"), "Stop", Some("y")),
    ]
    .join("\n");
    assert_eq!(
        last_firing(&log, "codex:"),
        Some(at("2026-11-12T10:00:00Z"))
    );
    assert_eq!(last_firing("", "codex:"), None);
}

#[test]
fn the_end_of_a_session_always_syncs_and_the_rest_waits_ten_minutes() {
    let now = Iso::parse("2026-11-12T12:00:00Z").unwrap();
    let ago = |ms: i64| Iso::from_millis(now.millis() - ms);
    let min = 60_000;
    assert!(hooks::due(Some("SessionEnd"), ago(min), now));
    assert!(!hooks::due(Some("Stop"), ago(9 * min), now));
    assert!(hooks::due(Some("Stop"), ago(10 * min), now));
    assert!(
        !hooks::due(None, ago(min), now),
        "an event we could not read counts as a Stop"
    );
    assert!(hooks::due(Some("Stop"), None, now), "never synced");
    assert!(
        hooks::due(Some("Stop"), ago(-5 * min), now),
        "a clock that went back must not block forever"
    );
}
