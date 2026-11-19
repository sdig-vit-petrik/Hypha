//! The period kernel: deadline arithmetic for a polled fixed period.
//!
//! A rewind is measured against the latest time seen, every expired period is counted,
//! and a deadline past the representable range exhausts the period until time moves back.

use crate::{Duration, Time};

/// What one call to [`Cadence::advance`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advance {
    /// True when `now` was earlier than the latest time seen: time moved backwards.
    pub rewound: bool,
    /// How many periods expired at or before `now`.
    pub expired: u128,
}

impl Advance {
    const IDLE: Self = Self { rewound: false, expired: 0 };
}

/// Tracks a running deadline for a fixed period.
///
/// The anchor is the latest time seen, which a rewind is measured against.
#[derive(Debug, Clone, Copy)]
pub struct Cadence {
    period: Duration,
    next: Time,
    /// False when `next` is saturated because the exact deadline is not representable.
    next_exact: bool,
    anchor: Time,
    /// True for a cadence on the epoch grid. A rewind keeps it on the grid.
    grid: bool,
}

impl Cadence {
    /// A cadence whose first deadline is one period after `now`.
    #[must_use]
    pub fn new(period: Duration, now: Time) -> Self {
        let (next, next_exact) = deadline_after(now, period);
        Self { period, next, next_exact, anchor: now, grid: false }
    }

    /// A cadence on the epoch grid: its deadlines are whole multiples of the period,
    /// the first strictly after `now`. A period that is not positive falls back to
    /// [`Cadence::new`].
    #[must_use]
    pub fn on_grid(period: Duration, now: Time) -> Self {
        let (next, next_exact) = grid_after(now, period);
        Self { period, next, next_exact, anchor: now, grid: true }
    }

    /// Restart the period from `now`.
    pub fn reset(&mut self, now: Time) {
        (self.next, self.next_exact) = deadline_after(now, self.period);
        self.anchor = now;
    }

    /// The configured period.
    #[must_use]
    pub const fn period(&self) -> Duration {
        self.period
    }

    /// The next deadline. Saturates at `i64::MAX`.
    #[must_use]
    pub const fn next_deadline(&self) -> Time {
        self.next
    }

    /// The instant a rewind is measured against.
    #[must_use]
    pub const fn anchor(&self) -> Time {
        self.anchor
    }

    /// The first deadline after a rewind to `now`: the next grid point for a grid
    /// cadence, else one period after `now`.
    fn first_after_rewind(&self, now: Time) -> (Time, bool) {
        if self.grid { grid_after(now, self.period) } else { deadline_after(now, self.period) }
    }

    /// Decide one step at `now` and update the state.
    // Division, not a loop, so a `now` far past the deadline costs one call.
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "Time and Duration operators saturate; the widened i128 period arithmetic cannot overflow"
    )]
    pub fn advance(&mut self, now: Time) -> Advance {
        let rewound = now < self.anchor;
        if !self.period.is_positive() {
            // Without a positive period nothing fires; a rewind is still reported.
            return Advance { rewound, expired: 0 };
        }

        self.anchor = now;
        if rewound {
            (self.next, self.next_exact) = self.first_after_rewind(now);
            return Advance { rewound: true, expired: 0 };
        }

        if !self.next_exact || now < self.next {
            return Advance::IDLE;
        }

        // Widen before subtracting: the range between two timestamps can exceed i64.
        let elapsed_nanos = i128::from(now.nanos()) - i128::from(self.next.nanos());
        let period_nanos = i128::from(self.period.nanos());
        let periods = elapsed_nanos / period_nanos + 1;
        let advanced = i128::from(self.next.nanos()) + periods * period_nanos;
        (self.next, self.next_exact) = exact_time(advanced);

        // `now >= next` on this path, so `periods >= 1`; the `unwrap_or(0)` is unreachable.
        Advance { rewound: false, expired: u128::try_from(periods).unwrap_or(0) }
    }
}

/// The deadline one period after `now`, and whether it is exact.
/// An inexact deadline saturates at the bound it overflowed toward.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "Time and Duration operators saturate; the checked add reports the overflow"
)]
fn deadline_after(now: Time, period: Duration) -> (Time, bool) {
    match now.nanos().checked_add(period.nanos()) {
        Some(nanos) => (Time::from_nanos(nanos), true),
        None => (now + period, false),
    }
}

/// The first multiple of `period` strictly after `now`, and whether it is exact.
///
/// A `now` on the grid is not its own deadline. A period that is not positive uses
/// [`deadline_after`].
#[allow(
    clippy::arithmetic_side_effects,
    reason = "the widened i128 grid arithmetic cannot overflow for an i64 period"
)]
fn grid_after(now: Time, period: Duration) -> (Time, bool) {
    if !period.is_positive() {
        return deadline_after(now, period);
    }
    let period_nanos = i128::from(period.nanos());
    let index = i128::from(now.nanos()).div_euclid(period_nanos) + 1;
    exact_time(index * period_nanos)
}

/// `nanos` as a time, and whether it is exact. A value past the range saturates at the
/// latest time.
fn exact_time(nanos: i128) -> (Time, bool) {
    i64::try_from(nanos)
        .map_or((Time::from_nanos(i64::MAX), false), |exact| (Time::from_nanos(exact), true))
}
