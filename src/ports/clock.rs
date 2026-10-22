//! The time source automation reads. It is injected, so that tests drive
//! ADR 0002 §9's bounds without waiting for them.

use crate::model::{TimestampMs, now_utc_ms};

/// Where the current time comes from.
pub trait Clock: Send + Sync {
    /// Now, in UTC epoch milliseconds.
    fn now_ms(&self) -> TimestampMs;
}

/// The system clock, read through [`now_utc_ms`].
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> TimestampMs {
        now_utc_ms()
    }
}
