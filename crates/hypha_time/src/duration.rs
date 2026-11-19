//! A signed time span with nanosecond resolution.

use core::fmt;
use core::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use core::time::Duration as StdDuration;

use crate::NANOS_PER_SECOND;
use crate::format::format_nanos;

/// A signed time span with nanosecond resolution.
///
/// Arithmetic saturates.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration {
    nanoseconds: i64,
}

impl Duration {
    /// A span of zero.
    pub const ZERO: Self = Self { nanoseconds: 0 };

    /// The span from a raw nanosecond count.
    #[must_use]
    pub const fn from_nanos(nanos: i64) -> Self {
        Self { nanoseconds: nanos }
    }

    /// The span from a microsecond count.
    #[must_use]
    pub const fn from_micros(micros: i64) -> Self {
        Self { nanoseconds: micros.saturating_mul(1_000) }
    }

    /// The span from a millisecond count.
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self { nanoseconds: millis.saturating_mul(1_000_000) }
    }

    /// The span from a whole second count.
    #[must_use]
    pub const fn from_secs(secs: i64) -> Self {
        Self { nanoseconds: secs.saturating_mul(NANOS_PER_SECOND) }
    }

    /// The span from a [`core::time::Duration`], saturating at [`i64::MAX`].
    #[must_use]
    pub fn from_std(span: StdDuration) -> Self {
        Self::from_nanos(i64::try_from(span.as_nanos()).unwrap_or(i64::MAX))
    }

    /// The raw nanosecond count.
    #[must_use]
    pub const fn nanos(self) -> i64 {
        self.nanoseconds
    }

    /// The span in seconds.
    #[must_use]
    pub fn seconds(self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let nanos = self.nanoseconds as f64;
        nanos / 1e9
    }

    /// True when the span is exactly zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.nanoseconds == 0
    }

    /// True when the span is greater than zero.
    #[must_use]
    pub const fn is_positive(self) -> bool {
        self.nanoseconds > 0
    }
}

impl Add for Duration {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self { nanoseconds: self.nanoseconds.saturating_add(other.nanoseconds) }
    }
}

impl Sub for Duration {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self { nanoseconds: self.nanoseconds.saturating_sub(other.nanoseconds) }
    }
}

impl Neg for Duration {
    type Output = Self;
    fn neg(self) -> Self {
        Self { nanoseconds: self.nanoseconds.saturating_neg() }
    }
}

impl AddAssign for Duration {
    fn add_assign(&mut self, other: Self) {
        self.nanoseconds = self.nanoseconds.saturating_add(other.nanoseconds);
    }
}

impl SubAssign for Duration {
    fn sub_assign(&mut self, other: Self) {
        self.nanoseconds = self.nanoseconds.saturating_sub(other.nanoseconds);
    }
}

impl fmt::Display for Duration {
    /// Seconds with nine fractional digits, for example `-1.500000000`.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        format_nanos(self.nanoseconds, out)
    }
}
