//! The agents' hooks. `files` edits their hook files; `run` holds the rules of the hook
//! itself. Pure: text in, text out; `commands` does the reading, writing and starting.
mod files;
mod run;

pub use files::{Plan, Problem, State, command, install, is_ours, state, uninstall};
pub use run::{INTERVAL_MS, MAX_INPUT, due, last_firing, log_line, parse_input, plain};
