//! Command line behaviour: help, exit codes and the answers that need no network.
//! Everything runs against a temporary data folder, never the real one.
use spendbranch::cli::{self, messages};
use spendbranch::infra::{lock, paths, store::Store};
use std::{path::Path, process::Command};

fn run_in(home: &Path, args: &[&str]) -> (i32, String) {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let home = home.to_str().unwrap().to_string();
    let env = move |k: &str| (k == "SPENDBRANCH_HOME").then(|| home.clone());
    let mut out = Vec::new();
    let code = cli::run(&args, &env, &mut out);
    (code, String::from_utf8(out).unwrap())
}

fn run(args: &[&str]) -> (i32, String) {
    run_in(tempfile::tempdir().unwrap().path(), args)
}

#[test]
fn no_command_or_help_flag_prints_the_help_and_exits_0() {
    for args in [&[][..], &["--help"], &["-h"], &["nope", "--help"]] {
        let (code, out) = run(args);
        assert_eq!(code, 0, "{args:?}");
        assert!(out.starts_with(&format!("spendbranch {}\n\nUsage:\n", cli::VERSION)));
        for line in [
            "spendbranch setup [--server URL] [--source claude-code|codex] [--relogin]",
            "spendbranch hooks uninstall codex",
            "spendbranch otel <source>",
            "spendbranch relay [--port N]",
            "spendbranch --version",
            "Prompts, responses, code, and paths never leave the device.",
        ] {
            assert!(out.contains(line), "help lacks: {line}");
        }
    }
}

#[test]
fn version_wins_over_everything() {
    assert_eq!(run(&["--version"]), (0, format!("{}\n", cli::VERSION)));
    assert_eq!(run(&["status", "-v"]), (0, format!("{}\n", cli::VERSION)));
}

#[test]
fn unknown_command_option_or_bad_value_is_a_usage_error() {
    let (code, out) = run(&["frobnicate"]);
    assert_eq!(code, 2);
    assert!(out.starts_with("Unknown command: frobnicate.\n\nspendbranch "));
    for args in [
        &["--nope"][..],
        &["login", "--server"],
        &["relay", "--port", "abc"],
        &["relay", "--port", "70000"],
    ] {
        let (code, out) = run(args);
        assert_eq!(code, 2, "{args:?}");
        assert!(out.contains("Usage:"), "{args:?}: help follows the message");
    }
}

#[test]
fn bad_arguments_exit_2_with_the_usage_line() {
    for (args, usage) in [
        (&["enable"][..], messages::USAGE_ENABLE),
        (&["enable", "gemini"], messages::USAGE_ENABLE),
        (&["disable"], messages::USAGE_DISABLE),
        (&["hooks"], messages::USAGE_HOOKS),
        (&["hooks", "install"], messages::USAGE_HOOKS),
        (&["hooks", "wipe", "codex"], messages::USAGE_HOOKS),
        (&["hooks", "install", "gemini"], messages::USAGE_HOOKS),
        (&["setup", "--source", "gemini"], messages::USAGE_SETUP),
        (&["otel"], messages::USAGE_OTEL),
        (&["otel", "gemini"], messages::USAGE_OTEL),
        (&["otel", "codex"], messages::OTEL_CODEX_REFUSED),
    ] {
        assert_eq!(run(args), (2, format!("{usage}\n")), "{args:?}");
    }
}

#[test]
fn source_aliases_are_accepted() {
    for args in [
        &["enable", "claude-code"][..],
        &["enable", "claude_code"],
        &["otel", "claude-code"],
        &["otel", "claude_code"],
    ] {
        assert_eq!(
            run(args),
            (1, format!("{}\n", messages::NOT_CONNECTED)),
            "{args:?}"
        );
    }
}

#[test]
fn commands_that_need_a_connection_say_so() {
    for args in [
        &["status"][..],
        &["sync"],
        &["enable", "codex"],
        &["disable", "codex"],
        &["relay"],
    ] {
        assert_eq!(
            run(args),
            (1, format!("{}\n", messages::NOT_CONNECTED)),
            "{args:?}"
        );
    }
}

#[test]
fn logout_when_not_connected_is_a_success() {
    assert_eq!(
        run(&["logout"]),
        (0, format!("{}\n", messages::ALREADY_DISCONNECTED))
    );
}

#[test]
fn hook_always_exits_0_and_writes_nothing() {
    for args in [
        &["hook", "claude-code"][..],
        &["hook", "codex", "Stop"],
        &["hook", "codex", "SessionEnd"],
    ] {
        assert_eq!(run(args), (0, String::new()), "{args:?}");
    }
    assert_eq!(run(&["hook", "gemini"]).0, 2);
}

#[test]
fn a_paired_device_gets_past_the_connection_check() {
    let dir = tempfile::tempdir().unwrap();
    Store::open(dir.path())
        .unwrap()
        .meta_set("device_id", "dev_synthetic")
        .unwrap();
    for args in [&["status"][..], &["logout"], &["sync"]] {
        assert_eq!(
            run_in(dir.path(), args),
            (1, format!("{}\n", messages::NOT_AVAILABLE)),
            "{args:?}"
        );
    }
}

#[test]
fn sync_yields_when_another_process_holds_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let _held = lock::try_acquire(dir.path()).unwrap().unwrap();
    assert_eq!(
        run_in(dir.path(), &["sync"]),
        (0, format!("{}\n", messages::SYNC_RUNNING))
    );
}

#[test]
fn an_unusable_data_folder_is_an_unexpected_error() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("a-file");
    std::fs::write(&blocker, "x").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
        .arg("status")
        .env("SPENDBRANCH_HOME", &blocker)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("unexpected error: "));
    assert!(out.stdout.is_empty());
}

#[test]
fn failure_messages_are_english_and_never_echo_unknown_input_verbatim_as_a_code_name() {
    for (code, text) in [
        (
            "NETWORK",
            "Could not reach the server. Check your network and server address (--server).",
        ),
        (
            "UNAUTHENTICATED",
            "This device was revoked or expired. Run `spendbranch login`.",
        ),
        ("PLAN_REQUIRED", "The organization has no active plan."),
        ("RATE_LIMITED", "The server requested a pause (rate limit)."),
        ("INTERNAL", "The server failed to receive the data."),
        (
            "SOMETHING_NEW",
            "The server rejected the request (SOMETHING_NEW).",
        ),
    ] {
        assert_eq!(messages::failure_message(code), text);
    }
    for code in [
        "COLLECTION_NOT_AUTHORIZED",
        "FIELDS_NOT_AUTHORIZED",
        "FORBIDDEN",
        "BEFORE_COLLECTION_WINDOW",
        "SESSION_OWNED",
    ] {
        let m = messages::failure_message(code);
        assert!(m.is_ascii() && !m.contains(code), "{code}: {m}");
    }
}

#[test]
fn data_folder_precedence() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    };
    let all = env(&[
        ("SPENDBRANCH_HOME", "/sb"),
        ("XDG_CONFIG_HOME", "/xdg"),
        ("HOME", "/h"),
        ("APPDATA", "/ad"),
    ]);
    assert_eq!(paths::data_dir(&all).unwrap(), Path::new("/sb"));
    let xdg = env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/h")]);
    assert_eq!(
        paths::data_dir(&xdg).unwrap(),
        Path::new("/xdg/spendbranch")
    );
    let empty_is_unset = env(&[
        ("SPENDBRANCH_HOME", ""),
        ("XDG_CONFIG_HOME", ""),
        ("HOME", "/h"),
        ("APPDATA", "/ad"),
    ]);
    let expected = if cfg!(windows) {
        Path::new("/ad").join("spendbranch")
    } else {
        Path::new("/h/.config/spendbranch").into()
    };
    assert_eq!(paths::data_dir(&empty_is_unset).unwrap(), expected);
    assert!(paths::data_dir(&env(&[])).is_err());
}
