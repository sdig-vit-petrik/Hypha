//! Safe, transport-independent cycle input capture and execution policy.

#![forbid(unsafe_code)]

mod capture;
mod history;
mod judgement;
mod replay_fidelity;
mod runtime;
mod supervision;

pub use capture::{
    BindingCapture, BindingCounters, BindingOptions, CaptureClock, CaptureMode, HeldReport,
    InputVerdict, QueueOverflowPolicy, held_report, judge_value,
};
pub use history::{CycleHistory, HistorySample, HistoryStats, SampleOrigin};
pub use judgement::{JudgementEvent, judgement_line};
pub use replay_fidelity::{
    ReplayFidelity, TIME_BASE_BOUND_PERIODS, counts_losses, is_short_history,
};
pub use runtime::{
    CycleHealth, CycleRuntime, CycleTime, EmissionBudget, OutputError, OutputFailurePolicy,
    StagedMessage,
};
pub use supervision::{FAULT_STAGNANT, FAULT_STALE, Supervision, SupervisionLimits};

/// The guard of `mutex`, taken as it is when a holder panicked. A shipped build aborts
/// on a panic, so no lock is ever poisoned there: the recovery serves test builds,
/// which unwind (`docs/rust-style.md`, "Locks").
#[inline]
pub(crate) fn locked<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
