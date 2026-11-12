//! `otel claude-code`: the environment that points Claude Code's OpenTelemetry at the relay.
use super::{Ctx, Result, account, say};
use crate::cli::messages as m;
use crate::infra::{paths, private_fs};

/// The settings, one `(name, value)` per line. No secret: the relay signs with the device key,
/// which never leaves this program.
const SETTINGS: [(&str, &str); 6] = [
    ("CLAUDE_CODE_ENABLE_TELEMETRY", "1"),
    ("OTEL_LOGS_EXPORTER", "otlp"),
    ("OTEL_METRICS_EXPORTER", "none"),
    ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json"),
    ("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", "http/json"),
    (
        "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        "http://127.0.0.1:4318/v1/logs",
    ),
];

fn render(windows: bool) -> String {
    SETTINGS
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
    let windows = cfg!(windows);
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
