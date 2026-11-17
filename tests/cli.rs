//! Command line behaviour: help, exit codes and the answers that need no network.
//! Everything runs against a temporary data folder, never the real one.
use spendbranch::cli::{self, Io, messages};
use spendbranch::infra::system::{Platform, Ran, System};
use spendbranch::infra::{lock, paths, store::Store};
use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
};

/// A system that never starts anything: no tools, no executable path, no keyboard.
struct Inert;

impl System for Inert {
    fn platform(&self) -> Platform {
        Platform::current()
    }
    fn exe(&self) -> io::Result<PathBuf> {
        Err(io::Error::other("no executable path in this mode"))
    }
    fn stdin_brief(&self) -> Option<Vec<u8>> {
        None
    }
    fn spawn_detached(&self, _: &Path, _: &[&str], _: &Path) -> io::Result<()> {
        Ok(())
    }
    fn run(&self, _: &str, _: &[&str], _: Option<&str>) -> io::Result<Ran> {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no tools in this mode",
        ))
    }
    fn stopped(&self) -> bool {
        true
    }
}

/// Runs the command line with only these variables set; `tty` says whether it is a terminal.
fn run_env(args: &[&str], vars: &[(&str, &str)], tty: bool) -> (i32, String) {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let vars: Vec<(String, String)> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let env = move |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let mut out = Vec::new();
    let code = cli::run_with(
        &args,
        &env,
        &mut Io {
            out: &mut out,
            input: &mut io::empty(),
            sleep: &|_| {},
            open_url: &|_| {},
            interactive: tty,
            system: &Inert,
        },
    );
    (code, String::from_utf8(out).unwrap())
}

fn run_in(home: &Path, args: &[&str]) -> (i32, String) {
    run_env(args, &[("SPENDBRANCH_HOME", home.to_str().unwrap())], false)
}

/// `spendbranch` alone, on a terminal, with a fresh data folder.
fn home_screen(vars: &[(&str, &str)]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let mut vars = vars.to_vec();
    vars.push(("SPENDBRANCH_HOME", dir.path().to_str().unwrap()));
    let (code, out) = run_env(&[], &vars, true);
    assert_eq!(code, 0);
    out
}

#[test]
fn the_owl_on_a_terminal_is_colored_only_where_truecolor_is_known_to_work() {
    let escapes = |vars: &[(&str, &str)]| home_screen(vars).contains('\x1b');
    assert!(escapes(&[("COLORTERM", "truecolor")]));
    assert!(escapes(&[
        ("COLORTERM", "24bit"),
        ("TERM", "xterm-256color")
    ]));
    assert!(!escapes(&[]), "no COLORTERM: no promise of truecolor");
    assert!(escapes(&[("WT_SESSION", "0c1d2e3f")]), "Windows Terminal");
    assert!(!escapes(&[("COLORTERM", "truecolor"), ("TERM", "dumb")]));
    assert!(!escapes(&[("COLORTERM", "truecolor"), ("NO_COLOR", "1")]));
    let plain = home_screen(&[]);
    assert!(
        plain.contains("( @ )"),
        "the owl keeps its shape without color"
    );
}

#[test]
fn the_start_screen_points_to_setup_and_to_the_help_without_creating_the_data_folder() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("data");
    let (code, out) = run_env(&[], &[("SPENDBRANCH_HOME", home.to_str().unwrap())], true);
    assert_eq!(code, 0);
    assert!(
        out.contains("Not connected") && out.contains("spendbranch setup"),
        "{out}"
    );
    assert!(out.contains("--help` for all commands"), "{out}");
    assert!(
        !home.exists(),
        "looking at the start screen created the data folder"
    );
}

#[test]
fn the_help_does_not_point_to_itself_and_has_no_owl() {
    let (_, out) = run_env(&["--help"], &[("COLORTERM", "truecolor")], true);
    assert!(!out.contains("for all commands"), "{out}");
    assert!(!out.contains("( @ )"), "{out}");
    assert!(out.contains("─"), "a terminal gets the rules");
}

fn run(args: &[&str]) -> (i32, String) {
    run_in(tempfile::tempdir().unwrap().path(), args)
}

#[test]
fn no_command_or_help_flag_prints_the_help_and_exits_0() {
    for args in [
        &[][..],
        &["--help"],
        &["-h"],
        &["nope", "--help"],
        &["hook", "--help"],
    ] {
        let (code, out) = run(args);
        assert_eq!(code, 0, "{args:?}");
        assert!(
            out.starts_with(&format!("spendbranch {} ", cli::VERSION)),
            "{out}"
        );
        for line in [
            "GET STARTED",
            "  setup                    connect this device",
            "  hooks uninstall <agent>",
            "  otel <agent>",
            "  relay [--port N]",
            "Prompts, responses, code, and paths",
            "Run `spendbranch <command> --help` for details.",
        ] {
            assert!(out.contains(line), "help lacks: {line}");
        }
        assert!(
            !out.contains('\x1b') && !out.contains('─'),
            "piped help is plain"
        );
    }
}

#[test]
fn a_command_with_help_shows_its_own_help_and_runs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (code, out) = run_in(dir.path(), &["setup", "--help"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("spendbranch setup [--server URL]"), "{out}");
    assert!(out.contains("--relogin"), "{out}");
    assert!(!dir.path().join("state.db").exists(), "setup ran");
    let (_, out) = run(&["logout", "-h"]);
    assert!(out.starts_with("spendbranch logout\n"), "{out}");
}

#[test]
fn version_wins_over_everything() {
    assert_eq!(run(&["--version"]), (0, format!("{}\n", cli::VERSION)));
    assert_eq!(run(&["status", "-v"]), (0, format!("{}\n", cli::VERSION)));
}

#[test]
fn unknown_command_option_or_bad_value_is_a_short_usage_error() {
    let (code, out) = run(&["stauts"]);
    assert_eq!(code, 2);
    assert_eq!(
        out,
        "error: unknown command 'stauts'\n  Did you mean `spendbranch status`?\n\nRun `spendbranch --help` to see all commands.\n"
    );
    let (code, out) = run(&["frobnicate"]);
    assert_eq!(code, 2);
    assert!(!out.contains("Did you mean"), "{out}");
    for args in [
        &["--nope"][..],
        &["login", "--server"],
        &["relay", "--port", "abc"],
        &["relay", "--port", "70000"],
    ] {
        let (code, out) = run(args);
        assert_eq!(code, 2, "{args:?}");
        assert!(out.starts_with("error: "), "{args:?}: {out}");
        assert!(out.contains("`spendbranch --help`"), "{args:?}: {out}");
        assert!(out.lines().count() <= 3, "{args:?}: the help is not dumped");
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
    // A bad server stops it before any network call; what matters is that the source is not
    // the reason (that would be the usage text).
    for source in ["claude-code", "claude_code", "codex"] {
        let (code, out) = run(&["setup", "--source", source, "--server", "not a url"]);
        assert_eq!(code, 2, "{source}");
        assert!(out.starts_with(messages::INVALID_SERVER), "{source}: {out}");
    }
    assert_eq!(
        run(&["setup", "--source", "gemini"]),
        (2, format!("{}\n", messages::USAGE_SETUP))
    );
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
