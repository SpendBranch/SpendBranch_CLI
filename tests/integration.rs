//! All integration tests in one binary: one link instead of one per file, which is most of the
//! build time. Each file stays a module. A new test file must be listed here (or get its own
//! `[[test]]` in Cargo.toml); `tests/architecture.rs` checks that none is left out.
#[path = "common/fake_server.rs"]
mod fake_server;
#[path = "common/rig.rs"]
mod rig;

#[path = "account.rs"]
mod account;
#[path = "api.rs"]
mod api;
#[path = "architecture.rs"]
mod architecture;
#[path = "auth.rs"]
mod auth;
#[path = "cli.rs"]
mod cli;
#[path = "contract.rs"]
mod contract;
#[path = "fixtures.rs"]
mod fixtures;
#[path = "hooks.rs"]
mod hooks;
#[path = "hooks_cli.rs"]
mod hooks_cli;
#[path = "infra.rs"]
mod infra;
#[path = "keys.rs"]
mod keys;
#[path = "otel.rs"]
mod otel;
#[path = "otlp_filter.rs"]
mod otlp_filter;
#[path = "otlp_route.rs"]
mod otlp_route;
#[path = "parsers.rs"]
mod parsers;
#[path = "readers_claude.rs"]
mod readers_claude;
#[path = "readers_codex.rs"]
mod readers_codex;
#[path = "readers_git.rs"]
mod readers_git;
#[path = "readers_hold.rs"]
mod readers_hold;
#[path = "relay.rs"]
mod relay;
#[path = "relay_rules.rs"]
mod relay_rules;
#[path = "scheduler.rs"]
mod scheduler;
#[path = "setup.rs"]
mod setup;
#[path = "sync.rs"]
mod sync;
#[path = "sync_rules.rs"]
mod sync_rules;
#[path = "tls.rs"]
mod tls;
#[path = "transport.rs"]
mod transport;
#[path = "uninstall.rs"]
mod uninstall;
