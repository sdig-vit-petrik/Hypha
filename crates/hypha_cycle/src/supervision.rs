//! Silence and stagnation checks of one supervised input. Owned by `BindingCapture`.
//!
//!
//! All times are the cycle's scheduling nanoseconds; only differences are used.

/// Bit set in `faults()` when the input is stale: its newest sample was sent longer
/// than the silence limit ago, or none arrived for that long since the start.
pub const FAULT_STALE: u32 = 1;
/// Bit set in `faults()` when the alive counter has not changed for the stagnation limit.
pub const FAULT_STAGNANT: u32 = 2;

/// Limits of the two checks, in nanoseconds. Zero turns that check off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SupervisionLimits {
    /// The most a sample may age after it was sent before the input is stale.
    pub silence_nanos: u64,
    /// Time without an alive change before the input is stagnant.
    pub stagnation_nanos: u64,
}

/// Staleness and stagnation state of one input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supervision {
    limits: SupervisionLimits,
    ignored: bool,
    start_nanos: Option<i64>,
    now_nanos: i64,
    last_received: u64,
    ever_accepted: bool,
    stale: bool,
    alive: Option<u64>,
    alive_changed_nanos: Option<i64>,
}

impl Supervision {
    /// Create a check with `limits`. It reports no fault before `on_capture`.
    #[must_use]
    pub const fn new(limits: SupervisionLimits) -> Self {
        Self {
            limits,
            ignored: false,
            start_nanos: None,
            now_nanos: 0,
            last_received: 0,
            ever_accepted: false,
            stale: false,
            alive: None,
            alive_changed_nanos: None,
        }
    }

    /// Configured silence and stagnation limits.
    #[must_use]
    pub const fn limits(&self) -> SupervisionLimits {
        self.limits
    }

    /// Record one iteration at framework time `now_nanos`.
    ///
    ///
    /// Any change of `received_total`, also a wrap, means a sample was accepted.
    pub fn on_capture(&mut self, now_nanos: i64, received_total: u64, stale: bool) {
        self.now_nanos = now_nanos;
        self.start_nanos = self.start_nanos.or(Some(now_nanos));
        let accepted_since_last_capture = received_total != self.last_received;
        self.last_received = received_total;
        self.ever_accepted |= accepted_since_last_capture;
        self.stale = stale;
    }

    /// Replace the binding's judgement of the last capture, after a re-judgement.
    pub fn set_stale(&mut self, stale: bool) {
        self.stale = stale;
    }

    /// Record the alive counter of the newest accepted sample.
    ///
    ///
    /// Does nothing before the first `on_capture`. Any change, also a wrap, restarts the
    /// stagnation timer.
    pub fn on_alive(&mut self, value: u64) {
        let no_capture_yet = self.start_nanos.is_none();
        if no_capture_yet {
            return;
        }
        let unchanged = self.alive == Some(value);
        if unchanged {
            return;
        }
        self.alive = Some(value);
        self.alive_changed_nanos = Some(self.now_nanos);
    }

    /// Override the fault. The fault bits of `faults()` do not change.
    pub const fn set_ignored(&mut self, ignored: bool) {
        self.ignored = ignored;
    }

    /// Whether the fault is overridden.
    #[must_use]
    pub const fn ignored(&self) -> bool {
        self.ignored
    }

    /// The real fault condition as a bit mask of `FAULT_*`, also when ignored.
    #[must_use]
    pub fn faults(&self) -> u32 {
        let stale_bit = if self.stale { FAULT_STALE } else { 0 };
        let stagnant_bit = if self.stagnant() { FAULT_STAGNANT } else { 0 };
        stale_bit | stagnant_bit
    }

    /// How long the alive value has not changed at the last capture, in nanoseconds.
    /// Zero before an alive value or when the clock went back.
    #[must_use]
    pub fn alive_unchanged_nanos(&self) -> u64 {
        let Some(changed) = self.alive_changed_nanos else {
            return 0;
        };
        u64::try_from(self.now_nanos.saturating_sub(changed)).unwrap_or(0)
    }

    /// Whether a fault counts: a fault is present and not ignored.
    #[must_use]
    pub fn faulted(&self) -> bool {
        self.faults() != 0 && !self.ignored
    }

    fn stagnant(&self) -> bool {
        let has_limit = self.limits.stagnation_nanos != 0;
        let Some(changed) = self.alive_changed_nanos else {
            return false;
        };
        let checked = has_limit && self.ever_accepted && !self.stale;
        checked && elapsed_at_least(self.now_nanos, changed, self.limits.stagnation_nanos)
    }
}

/// Whether `limit_nanos` or more passed from `since_nanos` to `now_nanos`; never for a
/// clock that went back.
fn elapsed_at_least(now_nanos: i64, since_nanos: i64, limit_nanos: u64) -> bool {
    let elapsed = now_nanos.saturating_sub(since_nanos);
    u64::try_from(elapsed).is_ok_and(|elapsed| elapsed >= limit_nanos)
}
