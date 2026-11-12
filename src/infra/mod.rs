//! The impure edge: files, locks, SQLite, sockets, processes and system tools. Nothing in the
//! pure modules (`sources`, `sync`, `contract`, `hooks`, `relay`, ...) may depend on it
//! (enforced by `tests/architecture.rs`).
pub mod agent_files;
pub mod browser;
pub mod clock;
pub mod hook_log;
pub mod http;
pub mod keys;
pub mod lock;
pub mod paths;
pub mod private_fs;
pub mod relay_server;
pub mod scheduler;
pub mod store;
pub mod system;
