//! `otel claude-code`: the environment file that points the agent at the relay.
#[path = "common/fake_server.rs"]
mod fake_server;
#[path = "common/rig.rs"]
mod rig;

use rig::{Rig, authorize, free_port};
use std::fs;

// --- `otel` ---------------------------------------------------------------------------

#[test]
fn otel_writes_a_private_environment_file_with_no_secret_in_it() {
    let rig = Rig::new();
    authorize(&rig);
    assert_eq!(rig.run(&["login"]).code, 0);
    let out = rig.run(&["otel", "claude-code"]);
    assert_eq!(out.code, 0, "{}", out.text);
    let file = rig.dir.path().join("otel-claude_code.env");
    let text = fs::read_to_string(&file).unwrap();
    for line in [
        "CLAUDE_CODE_ENABLE_TELEMETRY",
        "OTEL_LOGS_EXPORTER",
        "OTEL_METRICS_EXPORTER",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL",
    ] {
        assert!(text.contains(line), "{line}\n{text}");
    }
    assert!(
        text.contains("OTEL_METRICS_EXPORTER=none")
            || text.contains("OTEL_METRICS_EXPORTER = 'none'"),
        "{text}"
    );
    // The port and token the relay will check, picked for this install, never the well-known 4318.
    let store = rig.store();
    let port: u16 = store
        .meta_get("relay_port")
        .unwrap()
        .unwrap()
        .parse()
        .unwrap();
    let token = store.meta_get("relay_token").unwrap().unwrap();
    assert!(spendbranch::relay::PORTS.contains(&port), "{port}");
    assert!(
        text.contains(&format!("http://127.0.0.1:{port}/v1/logs")),
        "{text}"
    );
    assert!(
        token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
        "{token}"
    );
    assert!(
        text.contains(&format!(
            "export OTEL_EXPORTER_OTLP_LOGS_HEADERS=x-spendbranch-token={token}\n"
        )),
        "{text}"
    );
    // The device key is nowhere in it.
    let key = fs::read_to_string(rig.dir.path().join("keys").join("active.key")).or_else(|_| {
        fs::read_dir(rig.dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().contains("key"))
            .map(fs::read_to_string)
            .unwrap()
    });
    if let Ok(key) = key {
        let key = key.trim().to_owned();
        assert!(key.len() < 8 || !text.contains(&key));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(
        out.text.contains(&format!("source '{}'", file.display())),
        "{}",
        out.text
    );
    assert!(
        out.text.contains("keep `spendbranch relay` running"),
        "{}",
        out.text
    );
    assert!(out.text.contains("OTEL_LOG_USER_PROMPTS"), "{}", out.text);
    assert!(
        out.text.contains("restart a relay that is already running"),
        "{}",
        out.text
    );
    assert!(
        !text.contains("OTEL_EXPORTER_OTLP_HEADERS="),
        "the token goes only to the logs exporter: {text}"
    );
    // Again, over a longer old file: rewritten whole, with no tail of the old one, and with a
    // new token (running it again is how a squatted port or a leaked token is left behind).
    fs::write(&file, "x".repeat(5000)).unwrap();
    assert_eq!(rig.run(&["otel", "claude_code"]).code, 0);
    let again = fs::read_to_string(&file).unwrap();
    assert_eq!(again.lines().count(), text.lines().count(), "{again}");
    let new_token = rig.store().meta_get("relay_token").unwrap().unwrap();
    assert_ne!(new_token, token);
    assert!(again.contains(&new_token) && !again.contains('x'.to_string().repeat(10).as_str()));
}

#[test]
fn on_windows_otel_writes_a_powershell_file() {
    let mut rig = Rig::new();
    rig.system.platform = spendbranch::infra::system::Platform::Windows;
    authorize(&rig);
    assert_eq!(rig.run(&["login"]).code, 0);
    let out = rig.run(&["otel", "claude-code"]);
    assert_eq!(out.code, 0, "{}", out.text);
    let text = fs::read_to_string(rig.dir.path().join("otel-claude_code.ps1")).unwrap();
    assert!(text.contains("$env:OTEL_LOGS_EXPORTER = 'otlp'"), "{text}");
    assert!(
        text.contains("$env:OTEL_EXPORTER_OTLP_LOGS_HEADERS = 'x-spendbranch-token="),
        "{text}"
    );
    assert!(out.text.contains("Invoke-Expression"), "{}", out.text);
}

#[test]
fn otel_refuses_codex_and_an_unpaired_device() {
    let rig = Rig::new();
    let out = rig.run(&["otel", "codex"]);
    assert_eq!(out.code, 2);
    assert!(
        out.text
            .contains("Codex is not collected through OpenTelemetry"),
        "{}",
        out.text
    );
    let out = rig.run(&["otel", "claude-code"]);
    assert_eq!(out.code, 1);
    assert!(!rig.dir.path().join("otel-claude_code.env").exists());
}

#[test]
fn the_command_announces_the_address_and_ends_when_told_to_stop() {
    let rig = Rig::new();
    authorize(&rig);
    assert_eq!(rig.run(&["login"]).code, 0);
    assert_eq!(rig.run(&["otel", "claude-code"]).code, 0);
    let port = free_port();
    let out = rig.run(&["relay", "--port", &port.to_string()]); // the fake system says: stop now
    assert_eq!(out.code, 0, "{}", out.text);
    assert!(
        out.text.contains(&format!(
            "Relay listening on http://127.0.0.1:{port}/v1/logs and forwarding to {}",
            rig.server.base
        )),
        "{}",
        out.text
    );
    assert!(
        out.text
            .contains("point its OTEL_EXPORTER_OTLP_LOGS_ENDPOINT at this port"),
        "{}",
        out.text
    );
}
