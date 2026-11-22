//! The wall clock, as the app stamps things with it.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    now_nanos() as u64 / 1_000_000
}

/// Nanoseconds since the Unix epoch: for names that must not repeat.
pub fn now_nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}
