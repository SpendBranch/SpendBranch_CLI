//! `otel claude-code`: the environment that points Claude Code's OpenTelemetry at the relay.
use super::{Ctx, Result, account, say};
use crate::cli::messages as m;
use crate::infra::{paths, private_fs, system::Platform};
use crate::relay;

/// The settings, one `(name, value)` per line. No secret: the relay signs with the device key,
/// which never leaves this program.
fn settings() -> [(&'static str, String); 6] {
    [
        ("CLAUDE_CODE_ENABLE_TELEMETRY", "1".into()),
        ("OTEL_LOGS_EXPORTER", "otlp".into()),
        ("OTEL_METRICS_EXPORTER", "none".into()),
        ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json".into()),
        ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", "http/json".into()),
        (
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            format!("http://127.0.0.1:{}{}", relay::DEFAULT_PORT, relay::PATH),
        ),
    ]
}

fn render(windows: bool) -> String {
    settings()
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
    let windows = ctx.system.platform() == Platform::Windows;
    let dir = paths::data_dir(ctx.env)?;
    let file = dir.join(if windows {
        "otel-claude_code.ps1"
    } else {
        "otel-claude_code.env"
    });
    private_fs::write(&file, &render(windows))?;
    say(
        ctx,
        &m::otel_written(&file.display().to_string(), windows),
        0,
    )?;
    say(ctx, m::OTEL_WARNING, 0)
}
