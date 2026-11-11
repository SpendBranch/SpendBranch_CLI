//! SpendBranch CLI. Dependency rule: `cli -> commands -> infra`, while the pure modules
//! (`contract`, `sources`, and later `sync`) never import `infra`.
pub mod cli;
pub mod commands;
pub mod contract;
pub mod infra;
pub mod sources;
