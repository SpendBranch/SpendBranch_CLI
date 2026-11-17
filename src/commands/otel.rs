//! `otel claude-code`: the environment that points Claude Code's OpenTelemetry at the relay.
use super::{Ctx, Result, account, say};
use crate::cli::messages as m;
use crate::infra::{paths, private_fs, system::Platform};
use crate::relay;

/// Where `otel` keeps what it handed the agent, for `relay` to listen on and check.
pub(super) const PORT_KEY: &str = "relay_port";
pub(super) const TOKEN_KEY: &str = "relay_token";

/// The settings, one `(name, value)` per line. The token is the relay's own: it only says the
/// batch comes from the agent this file was loaded for. The device key never leaves this program.
fn settings(port: u16, token: &str) -> [(&'static str, String); 7] {
    [
        ("CLAUDE_CODE_ENABLE_TELEMETRY", "1".into()),
        ("OTEL_LOGS_EXPORTER", "otlp".into()),
        ("OTEL_METRICS_EXPORTER", "none".into()),
        ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json".into()),
        ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", "http/json".into()),
        (
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            format!("http://127.0.0.1:{port}{}", relay::PATH),
        ),
        (
            "OTEL_EXPORTER_OTLP_HEADERS",
            format!("{}={token}", relay::TOKEN_HEADER),
        ),
    ]
}

fn render(windows: bool, port: u16, token: &str) -> String {
    settings(port, token)
        .iter()
        .map(|(k, v)| {
            if windows {
                format!("$env:{k} = '{v}'\n")
            } else {
                format!("export {k}={v}\n")
            }
        })
        .collect()
}

pub(super) fn run(ctx: &mut Ctx) -> Result<i32> {
    let account = account::open(ctx)?;
    if !account.store.is_connected()? {
        return say(ctx, m::NOT_CONNECTED, 1);
    }
    // A new port and token on every run: running `otel` again is how a busy port is left behind.
    let mut random = [0u8; 34];
    getrandom::fill(&mut random).map_err(|_| std::io::Error::other("no randomness available"))?;
    let (token, port) = relay::new_endpoint(random);
    let windows = ctx.system.platform() == Platform::Windows;
    let dir = paths::data_dir(ctx.env)?;
    let file = dir.join(if windows {
        "otel-claude_code.ps1"
    } else {
        "otel-claude_code.env"
    });
    private_fs::write(&file, &render(windows, port, &token))?;
    account.store.meta_set(PORT_KEY, &port.to_string())?;
    account.store.meta_set(TOKEN_KEY, &token)?;
    say(
        ctx,
        &m::otel_written(&file.display().to_string(), windows),
        0,
    )?;
    say(ctx, m::OTEL_WARNING, 0)
}
