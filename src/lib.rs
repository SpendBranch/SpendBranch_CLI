//! SpendBranch CLI. Dependency rule: `cli -> commands -> infra`. The pure modules (`contract`,
//! `sources`, `sync`, `auth`, `api`, `hooks`, `relay`, `scheduler`, `cli::{args, messages,
//! render}`) never import `infra` or touch files, sockets, processes or the environment;
//! `tests/architecture.rs` enforces it, and keeps `commands` off files, sockets and SQLite.
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
