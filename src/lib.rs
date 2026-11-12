//! SpendBranch CLI. Dependency rule: `cli -> commands -> infra`, while the pure modules
//! (`contract`, `sources`, and later `sync`) never import `infra`.
#![forbid(unsafe_code)]
// Production code must not be able to panic: a hook that crashes disturbs the agent.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::dbg_macro,
        clippy::print_stdout,
        clippy::print_stderr
    )
)]

pub mod api;
pub mod auth;
pub mod cli;
pub mod commands;
pub mod contract;
pub mod env;
pub mod hooks;
pub mod infra;
pub mod readers;
pub mod relay;
pub mod scheduler;
pub mod sources;
pub mod sync;
