//! Fixed-period work, driven by the caller's loop.
//!
//! `Time` and `Duration` arithmetic saturates by construction, which the lint
//! cannot see through an operator implementation.
use hypha_time::{Cadence, Duration, Time};

/// What one poll of a [`Timer`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    /// How many periods expired at or before the polled time, saturated at `u32::MAX`.
    pub fired: u32,
    /// True when the polled time was earlier than the latest time seen; nothing fires.
    pub rewound: bool,
}

/// Tracks when a period expires.
///
/// The timer holds no thread and no callback; the caller polls it and does the work.
/// Framework time can move backwards (a replayed `/clock`): a poll earlier than the
/// latest time seen restarts the period from that time.
#[derive(Debug, Clone, Copy)]
pub struct Timer {
    cadence: Cadence,
    missed: u64,
    /// True for a grid timer whose phase is not set yet; it fires nothing until then.
    awaiting_anchor: bool,
    /// True for a timer on the epoch grid.
    grid: bool,
}

impl Timer {
    /// A timer with no missed period yet.
    const fn from_parts(cadence: Cadence, awaiting_anchor: bool, grid: bool) -> Self {
        Self { cadence, missed: 0, awaiting_anchor, grid }
    }

    /// A timer whose first period expires one period after `now`.
    /// A period that is not positive yields a timer that never fires.
    #[must_use]
    pub fn new(period: Duration, now: Time) -> Self {
        Self::from_parts(Cadence::new(period, now), false, false)
    }

    /// A timer whose deadlines are whole multiples of `period`, with its phase
    /// taken at `now`: the first deadline is the first multiple strictly after `now`.
    #[must_use]
    pub fn on_grid(period: Duration, now: Time) -> Self {
        Self::from_parts(Cadence::on_grid(period, now), false, true)
    }

    /// A grid timer whose phase is not known yet. It fires nothing until
    /// [`Timer::anchor_grid`] gives it the first time that counts.
    #[must_use]
    pub fn awaiting_grid(period: Duration) -> Self {
        Self::from_parts(Cadence::new(period, Time::ZERO), true, true)
    }

    /// Set the phase of an [`Timer::awaiting_grid`] timer from `now`. It fires
    /// nothing at the anchor. An anchored timer is unchanged.
    pub fn anchor_grid(&mut self, now: Time) {
        if !self.awaiting_anchor {
            return;
        }
        self.cadence = Cadence::on_grid(self.cadence.period(), now);
        self.awaiting_anchor = false;
    }

    /// True while a grid timer waits for its phase. Its observations fire nothing.
    #[must_use]
    pub const fn awaiting_anchor(&self) -> bool {
        self.awaiting_anchor
    }

    /// True for a timer on the epoch grid.
    #[must_use]
    pub const fn is_grid(&self) -> bool {
        self.grid
    }

    /// How many periods have expired since the last call.
    ///
    /// Extra periods are counted in [`Timer::missed_count`] as well. A `now` earlier
    /// than the latest time seen is a rewind: it fires nothing, counts no missed period,
    /// and makes the next period expire one period after `now`.
    pub fn poll(&mut self, now: Time) -> u32 {
        self.observe(now).fired
    }

    /// Poll as [`Timer::poll`] does, and also report whether `now` was a rewind.
    pub fn observe(&mut self, now: Time) -> Observation {
        if self.awaiting_anchor {
            return Observation { fired: 0, rewound: false };
        }
        let advance = self.cadence.advance(now);
        let extra_periods = advance.expired.saturating_sub(1);
        self.missed = self.missed.wrapping_add(u64::try_from(extra_periods).unwrap_or(u64::MAX));
        Observation {
            fired: u32::try_from(advance.expired).unwrap_or(u32::MAX),
            rewound: advance.rewound,
        }
    }

    /// Restart the period from `now`.
    pub fn reset(&mut self, now: Time) {
        self.cadence.reset(now);
    }

    /// The configured period.
    #[must_use]
    pub const fn period(&self) -> Duration {
        self.cadence.period()
    }

    /// When the next period expires. Returns `i64::MAX` nanoseconds when no later
    /// deadline is representable.
    #[must_use]
    pub const fn next_due(&self) -> Time {
        self.cadence.next_deadline()
    }

    /// How many periods went by without the caller noticing.
    #[must_use]
    pub const fn missed_count(&self) -> u64 {
        self.missed
    }
}
