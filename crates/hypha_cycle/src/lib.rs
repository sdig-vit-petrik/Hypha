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
