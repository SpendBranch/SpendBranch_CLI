//! The process environment as a parameter, so nothing reads global state and tests never
//! touch the real environment.

/// Reads one environment variable.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<String>;
