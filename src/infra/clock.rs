//! The time. A plain function; code that tests must steer takes a `Clock` closure instead.
use std::time::{SystemTime, UNIX_EPOCH};

/// What tells the time, in unix milliseconds: `now_ms` for real, a closure in tests.
pub type Clock = Box<dyn Fn() -> i64>;

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
