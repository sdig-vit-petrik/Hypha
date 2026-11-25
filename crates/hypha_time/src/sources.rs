//! The operating-system clocks and the hand-advanced clock.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use rustix::time::{ClockId, clock_gettime};

use crate::{Duration, Time};

/// A clock advanced by hand, for tests and playback.
///
/// Every method takes `&self`, so the registry can move it from any thread.
#[derive(Debug)]
pub(crate) struct SimulatedClock {
    nanos: AtomicI64,
    /// True once the clock holds a value somebody chose, not a placeholder start.
    has_value: AtomicBool,
}

impl SimulatedClock {
    /// A simulated clock reading `start`; `has_value` says whether `start` was chosen.
    pub(crate) fn starting_at(start: Time, has_value: bool) -> Self {
        Self { nanos: AtomicI64::new(start.nanos()), has_value: AtomicBool::new(has_value) }
    }

    /// Move the clock to `instant`.
    pub(crate) fn set(&self, instant: Time) {
        self.nanos.store(instant.nanos(), Ordering::Release);
        self.has_value.store(true, Ordering::Release);
    }

    /// Move the clock forward by `span`, saturating at the latest time.
    pub(crate) fn advance(&self, span: Duration) {
        self.has_value.store(true, Ordering::Release);
        // A compare-exchange loop, not `fetch_update`: newer Rust deprecates that name, and
        // its successor `try_update` is newer than the minimum supported Rust.
        let mut current = self.nanos.load(Ordering::Acquire);
        loop {
            let next = current.saturating_add(span.nanos());
            match self.nanos.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }

    /// The reading the clock holds now.
    pub(crate) fn now(&self) -> Time {
        Time::from_nanos(self.nanos.load(Ordering::Acquire))
    }

    /// Whether the clock holds a value somebody chose.
    pub(crate) fn has_value(&self) -> bool {
        self.has_value.load(Ordering::Acquire)
    }
}

/// The host's monotonic clock (`CLOCK_MONOTONIC`).
///
/// Ignores simulated time. The origin is the system's, the same for every process of
/// the host, so a reading of one process compares with a reading of another: a
/// subscriber compares a frame's send stamp with its own clock. The origin itself
/// means nothing: use differences. Zero on a system that is not Unix.
#[must_use]
pub fn steady_now() -> Time {
    #[cfg(unix)]
    {
        let reading = clock_gettime(ClockId::Monotonic);
        let seconds = i128::from(reading.tv_sec);
        let nanos = i128::from(reading.tv_nsec);
        let total = seconds.saturating_mul(1_000_000_000).saturating_add(nanos);
        let saturated_at = if total.is_negative() { i64::MIN } else { i64::MAX };
        Time::from_nanos(i64::try_from(total).unwrap_or(saturated_at))
    }
    #[cfg(not(unix))]
    Time::ZERO
}

/// The UTC wall clock, as the system reports it.
///
/// A clock set before the Unix epoch yields [`Time::ZERO`]; use [`try_wall_now`] where
/// that matters. It steps when the system clock steps.
#[must_use]
pub fn wall_now() -> Time {
    try_wall_now().unwrap_or(Time::ZERO)
}

/// The UTC wall clock, or `None` when it reads before the Unix epoch.
#[must_use]
pub fn try_wall_now() -> Option<Time> {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some(Time::from_nanos(Duration::from_std(since_epoch).nanos()))
}
