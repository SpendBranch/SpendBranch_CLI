//! The impure edge: files, locks, SQLite. Nothing in `sources`, `sync` or `contract`
//! may depend on this module (enforced by `tests/architecture.rs`).
pub mod clock;
pub mod hook_log;
pub mod http;
pub mod keys;
pub mod lock;
pub mod paths;
pub mod private_fs;
pub mod store;
