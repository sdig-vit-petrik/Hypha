//! Bounded dispatch policy.
//!
//! How much work one execution turn may do and which registration runs next. No
//! transport, no pointer, no wait.

use std::fmt;
use std::time::Duration;

/// The default number of samples one attachment takes in one turn. Rejected
/// frames count against it.
pub const DEFAULT_BATCH_SIZE: u32 = 8;

/// The default number of work items before the loop checks native readiness again.
pub const DEFAULT_DISPATCH_BUDGET: u32 = 64;

/// The default number of registrations one execution configuration accepts,
/// including the registrations the framework adds itself.
pub const DEFAULT_MAX_REGISTRATIONS: u32 = 128;

/// Why a set of limits was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitError {
    /// A limit was zero. A zero limit would stop all work.
    Zero,
    /// The batch size was larger than the dispatch budget.
    BatchAboveBudget,
    /// The registration count was above the supported maximum.
    TooManyRegistrations,
}

impl fmt::Display for LimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => f.write_str("a dispatch limit was zero"),
            Self::BatchAboveBudget => {
                f.write_str("the batch size was larger than the dispatch budget")
            }
            Self::TooManyRegistrations => {
                f.write_str("the registration count was above the supported maximum")
            }
        }
    }
}

impl std::error::Error for LimitError {}

/// The largest registration count this policy supports.
pub const REGISTRATION_CEILING: u32 = 4096;

/// Validated execution limits.
///
/// Engineering defaults, not safety-derived limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchLimits {
    batch_size: u32,
    dispatch_budget: u32,
    max_registrations: u32,
}

impl Default for DispatchLimits {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
            dispatch_budget: DEFAULT_DISPATCH_BUDGET,
            max_registrations: DEFAULT_MAX_REGISTRATIONS,
        }
    }
}

impl DispatchLimits {
    /// Build limits, refusing values the dispatcher cannot honour.
    /// A limit is never clamped.
    ///
    /// # Errors
    /// Returns [`LimitError`] when a limit is zero, when the batch size is
    /// above the dispatch budget, or when the registration count is above
    /// [`REGISTRATION_CEILING`].
    pub const fn new(
        batch_size: u32,
        dispatch_budget: u32,
        max_registrations: u32,
    ) -> Result<Self, LimitError> {
        if batch_size == 0 || dispatch_budget == 0 || max_registrations == 0 {
            return Err(LimitError::Zero);
        }
        if batch_size > dispatch_budget {
            return Err(LimitError::BatchAboveBudget);
        }
        if max_registrations > REGISTRATION_CEILING {
            return Err(LimitError::TooManyRegistrations);
        }
        Ok(Self { batch_size, dispatch_budget, max_registrations })
    }

    /// Samples one attachment takes in one turn.
    #[must_use]
    pub const fn batch_size(&self) -> u32 {
        self.batch_size
    }

    /// Work items before the loop rechecks native readiness.
    #[must_use]
    pub const fn dispatch_budget(&self) -> u32 {
        self.dispatch_budget
    }

    /// Registrations one configuration accepts.
    #[must_use]
    pub const fn max_registrations(&self) -> u32 {
        self.max_registrations
    }
}

/// The time a callback may take before it is slow, when nothing says otherwise, and the
/// budget of a subscription: 10 ms of host time. A timer or a cycle is judged against its
/// own period instead.
pub const DEFAULT_CALLBACK_BUDGET_NANOS: u64 = 10_000_000;

/// What one callback is judged against: the time it may take and whether crossing it is
/// written to the log. A slow callback is counted whatever `warns` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallbackBudget {
    /// A callback that takes longer than this is slow.
    pub nanos: u64,
    /// Whether a slow callback writes a line.
    pub warns: bool,
}

/// The budget of a callback from the node's `[log] slow_callback_ms` and, for a timer or a
/// cycle, its period.
///
/// A configured value is the budget of every callback of the node. Zero writes no line
/// and leaves the budget at what it would be without the key, so the counter goes on.
/// Without the key a timer or a cycle has its period, and a subscription
/// [`DEFAULT_CALLBACK_BUDGET_NANOS`].
#[must_use]
pub fn callback_budget(configured_ms: Option<u32>, period_nanos: Option<u64>) -> CallbackBudget {
    let own = period_nanos.filter(|period| *period > 0).unwrap_or(DEFAULT_CALLBACK_BUDGET_NANOS);
    match configured_ms {
        None => CallbackBudget { nanos: own, warns: true },
        Some(0) => CallbackBudget { nanos: own, warns: false },
        Some(millis) => {
            CallbackBudget { nanos: u64::from(millis).saturating_mul(1_000_000), warns: true }
        }
    }
}

/// Nanoseconds as milliseconds with the fraction that matters, rounded up to a
/// microsecond: `10`, `2.5`, `10.001`.
fn millis_text(nanos: u64) -> String {
    let micros = nanos.div_ceil(1_000);
    let (millis, fraction) = (micros / 1_000, micros % 1_000);
    if fraction == 0 {
        return millis.to_string();
    }
    let fraction = format!("{fraction:03}");
    format!("{millis}.{}", fraction.trim_end_matches('0'))
}

/// The warning for a slow callback of `subject` (`topic '/x'`, `timer 100 ms`,
/// `cycle 'ekf'`): how long it took, the budget it crossed and how many warnings the rate
/// limit held back since the last line.
#[must_use]
pub fn slow_callback_line(
    subject: &str,
    elapsed_nanos: u64,
    budget_nanos: u64,
    suppressed: u64,
) -> String {
    format!(
        "hypha: callback of {subject} took {} ms, budget {} ms, {suppressed} warnings suppressed",
        millis_text(elapsed_nanos),
        millis_text(budget_nanos)
    )
}

/// Whether one callback's measured duration crosses the slow-callback
/// threshold. A zero threshold disables the warning.
#[must_use]
pub fn is_slow_callback(elapsed: Duration, warn_nanos: u64) -> bool {
    warn_nanos != 0 && elapsed.as_nanos() > u128::from(warn_nanos)
}

/// Whether a transport failure starts on this turn, so it should be reported.
///
/// A failure is reported once, when it begins.
#[must_use]
pub fn transport_failure_starts(was_failing: bool, failed: bool) -> bool {
    failed && !was_failing
}

/// Whether a slow-callback warning for one attachment may be printed again.
///
/// `elapsed_since_last` is [`None`] when no warning has been printed yet.
#[must_use]
pub fn is_warning_due(elapsed_since_last: Option<Duration>, interval: Duration) -> bool {
    elapsed_since_last.is_none_or(|elapsed| elapsed >= interval)
}

/// One step taken while draining a subscription's queue for one turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStep {
    /// A frame was taken and delivered to the application callback.
    Accepted,
    /// A frame was taken but refused before it reached the callback.
    /// It still counts against the batch.
    Refused,
    /// The queue had nothing more to take this turn.
    Exhausted,
}

/// How many frames one attachment has taken in the current turn.
///
/// A turn takes at most `batch_size` frames.
#[derive(Debug)]
pub struct BatchTurn {
    taken: u32,
    batch_size: u32,
}

impl BatchTurn {
    /// Start a turn that may take up to `batch_size` frames.
    #[must_use]
    pub const fn new(batch_size: u32) -> Self {
        Self { taken: 0, batch_size }
    }

    /// Record one step. Returns `true` when the loop should take another
    /// step, `false` when the turn is full or the queue is exhausted.
    pub fn record(&mut self, step: BatchStep) -> bool {
        match step {
            BatchStep::Accepted | BatchStep::Refused => {
                self.taken = self.taken.saturating_add(1);
                !self.is_full()
            }
            BatchStep::Exhausted => false,
        }
    }

    /// True when this turn took its whole batch.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.taken >= self.batch_size
    }
}
