//! A point in time with nanosecond resolution.

use core::fmt;
use core::ops::{Add, AddAssign, Sub};

use crate::Duration;
use crate::format::format_nanos;

/// A point in time with nanosecond resolution.
///
///
/// The epoch follows the clock source: wall time counts from the Unix epoch, monotonic
/// and simulated time from an arbitrary origin. Arithmetic saturates.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time {
    nanoseconds: i64,
}

impl Time {
    /// The zero instant, which means "time is not yet known".
    pub const ZERO: Self = Self { nanoseconds: 0 };

    /// The instant from a raw nanosecond count.
    #[must_use]
    pub const fn from_nanos(nanos: i64) -> Self {
        Self { nanoseconds: nanos }
    }

    /// The raw nanosecond count.
    #[must_use]
    pub const fn nanos(self) -> i64 {
        self.nanoseconds
    }

    /// This time as a UTC date. Meaningful for wall-clock time only.
    #[must_use]
    pub const fn utc(self) -> crate::Utc {
        crate::Utc::new(self.nanoseconds)
    }

    /// The instant in seconds.
    #[must_use]
    pub fn seconds(self) -> f64 {
        Duration::from_nanos(self.nanoseconds).seconds()
    }

    /// True when the instant is zero.
    ///
    /// Zero is the sentinel for "time is not yet known".
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.nanoseconds == 0
    }
}

impl Add<Duration> for Time {
    type Output = Self;
    fn add(self, span: Duration) -> Self {
        Self { nanoseconds: self.nanoseconds.saturating_add(span.nanos()) }
    }
}

impl Sub<Duration> for Time {
    type Output = Self;
    fn sub(self, span: Duration) -> Self {
        Self { nanoseconds: self.nanoseconds.saturating_sub(span.nanos()) }
    }
}

impl Sub for Time {
    type Output = Duration;
    /// The span from `other` to `self`.
    fn sub(self, other: Self) -> Duration {
        Duration::from_nanos(self.nanoseconds.saturating_sub(other.nanoseconds))
    }
}

impl AddAssign<Duration> for Time {
    fn add_assign(&mut self, span: Duration) {
        self.nanoseconds = self.nanoseconds.saturating_add(span.nanos());
    }
}

impl fmt::Display for Time {
    /// Seconds with nine fractional digits, for example `12.250000000`.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_nanos(self.nanoseconds, out)
    }
}
