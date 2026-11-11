//! SpendBranch CLI. Dependency rule: `cli -> commands -> infra`, while the pure modules
//! (`sources`, and later `contract` and `sync`) never import `infra`.
pub mod cli;
pub mod commands;
pub mod infra;
pub mod sources;
