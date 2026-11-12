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
    // status, logout and sync work on what is saved: a device id with no server or key is not usable
    let (code, out) = run_in(dir.path(), &["sync"]);
    assert_eq!(code, 1);
    assert!(
        out.starts_with("The saved connection is not valid."),
        "{out}"
    );
    let (code, out) = run_in(dir.path(), &["status"]);
    assert_eq!(code, 1);
    assert!(
        out.starts_with("The saved connection is not valid."),
        "{out}"
    );
    let (code, out) = run_in(dir.path(), &["logout"]);
    assert_eq!(code, 0);
    assert!(out.contains(messages::LOGOUT_LOCAL_DELETED), "{out}");
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
    let env = |pairs: &[(&str, &str)]| {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    };
    // absolute on every platform (`/sb` has no drive letter on Windows)
    let root = std::env::temp_dir();
    let p = |name: &str| root.join(name).to_string_lossy().into_owned();
    let (sb, xdg_dir, h, ad) = (p("sb"), p("xdg"), p("h"), p("ad"));
    let all = env(&[
        ("SPENDBRANCH_HOME", &sb),
        ("XDG_CONFIG_HOME", &xdg_dir),
        ("HOME", &h),
        ("APPDATA", &ad),
    ]);
    assert_eq!(paths::data_dir(&all).unwrap(), Path::new(&sb));
    let xdg = env(&[("XDG_CONFIG_HOME", &xdg_dir), ("HOME", &h)]);
    assert_eq!(
        paths::data_dir(&xdg).unwrap(),
        Path::new(&xdg_dir).join("spendbranch")
    );
    let empty_is_unset = env(&[
        ("SPENDBRANCH_HOME", ""),
        ("XDG_CONFIG_HOME", ""),
        ("HOME", &h),
        ("APPDATA", &ad),
    ]);
    let expected = if cfg!(windows) {
        Path::new(&ad).join("spendbranch")
    } else {
        Path::new(&h).join(".config/spendbranch")
    };
    assert_eq!(paths::data_dir(&empty_is_unset).unwrap(), expected);
    // a relative SPENDBRANCH_HOME is refused; a relative XDG_CONFIG_HOME is ignored
    let relative = env(&[
        ("SPENDBRANCH_HOME", "state"),
        ("HOME", &h),
        ("APPDATA", &ad),
    ]);
    assert!(
        paths::data_dir(&relative)
            .unwrap_err()
            .to_string()
            .contains("absolute")
    );
    let relative_xdg = env(&[("XDG_CONFIG_HOME", "cfg"), ("HOME", &h), ("APPDATA", &ad)]);
    assert_eq!(paths::data_dir(&relative_xdg).unwrap(), expected);
    assert!(paths::data_dir(&env(&[])).is_err());
}

#[test]
fn a_code_from_the_server_is_echoed_only_if_it_looks_like_a_code() {
    // The server's text is not ours: a terminal would obey an escape sequence hidden in it.
    assert_eq!(
        messages::failure_message("NEW_CODE_42"),
        "The server rejected the request (NEW_CODE_42)."
    );
    for hostile in [
        "\u{1b}[31mred",
        "line\nbreak",
        "lower_case",
        "has space",
        "",
        &"A".repeat(65),
        "\u{202e}RTL",
        "ÉCOLE",
    ] {
        let m = messages::failure_message(hostile);
        assert_eq!(m, "The server rejected the request.", "{hostile:?}");
        assert!(!m.contains(hostile) || hostile.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn an_argument_that_is_not_text_is_a_usage_error_not_a_crash() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &[b"\xff\xfe".as_slice()][..],
        &[b"status", b"\xff"],
        &[b"--server", b"\xc3\x28"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
            .args(args.iter().map(|a| std::ffi::OsStr::from_bytes(a)))
            .env("SPENDBRANCH_HOME", dir.path().join("data"))
            .output()
            .unwrap();
        let code = out.status.code();
        assert!(
            matches!(code, Some(0..=2)),
            "{args:?} exited with {code:?} (101 would be a panic)"
        );
        assert!(
            out.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn setup_takes_the_same_source_spellings_as_enable() {
    for source in ["claude-code", "claude_code", "codex"] {
        let (code, out) = run(&["setup", "--source", source]);
        assert_eq!(
            (code, out.as_str()),
            (1, format!("{}\n", messages::NOT_AVAILABLE).as_str()),
            "{source}"
        );
    }
}

/// `eprintln!` panics (exit 101) when stderr cannot be written; `/dev/full` fails every write.
#[cfg(target_os = "linux")]
#[test]
fn an_unwritable_stderr_does_not_turn_an_error_into_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("a-file");
    std::fs::write(&blocker, "x").unwrap();
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
        .arg("status")
        .env("SPENDBRANCH_HOME", &blocker)
        .stderr(full)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}

#[test]
fn a_relative_data_folder_is_refused_with_a_clear_message() {
    let out = Command::new(env!("CARGO_BIN_EXE_spendbranch"))
        .arg("status")
        .env("SPENDBRANCH_HOME", "state")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("SPENDBRANCH_HOME must be an absolute path")
    );
}
