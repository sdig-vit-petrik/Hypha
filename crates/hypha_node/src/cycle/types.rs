//! C ABI value types of the cycle surface.

use std::os::raw::c_void;

use super::image::{HyphaCycleEmission, HyphaCycleImage};

/// Freshness verdict: the capture clock has admitted no sample since the binding was
/// created.
pub const HYPHA_CYCLE_VERDICT_NEVER_ARRIVED: u32 = 0;
/// The newest admitted sample is within the silence limit (for a latest binding with
/// none retained, its held sample). A queued binding with no new sample is usable.
pub const HYPHA_CYCLE_VERDICT_USABLE: u32 = 1;
/// The newest admitted sample is older than the silence limit or stamped after the
/// capture clock.
pub const HYPHA_CYCLE_VERDICT_STALE: u32 = 2;

/// Fault bit: the input is stale, its newest sample older than the silence limit.
pub const HYPHA_FAULT_STALE: u32 = 1;
/// Fault bit: the alive counter unchanged within the stagnation limit.
pub const HYPHA_FAULT_STAGNANT: u32 = 2;
const _: () = assert!(HYPHA_FAULT_STALE == hypha_cycle::FAULT_STALE);
const _: () = assert!(HYPHA_FAULT_STAGNANT == hypha_cycle::FAULT_STAGNANT);

/// Silence and stagnation limits of one supervised input, in nanoseconds.
/// Zero turns that check off.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaSupervisionLimits {
    /// Time without an accepted sample before the input is silent.
    pub silence_nanos: i64,
    /// Time without an alive change before the input is stagnant.
    pub stagnation_nanos: i64,
}

/// The settings of a cycle that drives `/clock` for a simulator.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HyphaClockDriverOptions {
    /// The first `/clock` value, in nanoseconds, sent with no step. Zero or more, and below 2^32 seconds: `/clock` carries seconds in 32 bits.
    pub start_nanos: i64,
    /// A required participant silent this long fails the run. Positive.
    pub timeout_nanos: u64,
    /// Simulated seconds per host second. Zero steps as fast as the participants allow;
    /// otherwise positive and finite.
    pub real_time_factor: f64,
}

/// The supervision state of one input binding.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaSupervisionReport {
    /// Bit mask of `HYPHA_FAULT_*`, also when ignored. Zero when unsupervised.
    pub faults: u32,
    /// Nonzero when the fault is overridden.
    pub ignored: u8,
}

/// How one cycle input binding consumes retained samples.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyphaCycleInputMode {
    /// Select the newest accepted sample.
    Latest = 0,
    /// Consume retained samples with an independent cursor.
    Queued = 1,
    /// Queued, newest only: a step examines the newest pending sample and drops the older
    /// ones as work-limit drops. A trigger in this mode steps on the newest of the samples
    /// due together and passes the older ones, counted the same way: on host time every
    /// sample pending in a turn, under simulated time the samples of one stamp
    /// (execution-model.md §6.6). `max_samples_per_iteration` is the queue the binding
    /// keeps between steps.
    Newest = 2,
}

impl TryFrom<u32> for HyphaCycleInputMode {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Latest),
            1 => Ok(Self::Queued),
            2 => Ok(Self::Newest),
            _ => Err(()),
        }
    }
}

/// Handling of one output publication refusal.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyphaCycleOutputFailure {
    /// Fail the owning `WaitSet` on any publication refusal.
    FailRun = 0,
    /// Drop and count only pool exhaustion; fail on every other refusal.
    DropPoolExhausted = 1,
}

impl TryFrom<u32> for HyphaCycleOutputFailure {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::FailRun),
            1 => Ok(Self::DropPoolExhausted),
            _ => Err(()),
        }
    }
}

/// The SDK default of `HyphaCycleBindingOptions::max_samples_per_iteration` for a
/// queued input that sets no limit of its own: 8 samples per step.
pub const HYPHA_DEFAULT_ARRIVALS_PER_STEP: usize = 8;

/// Capture policy for one cycle input binding.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HyphaCycleBindingOptions {
    /// Latest or queued capture, a `HyphaCycleInputMode`. Another value is
    /// `HYPHA_STATUS_INVALID_ARGUMENT`.
    pub mode: u32,
    /// Queued samples examined per iteration. Must be nonzero for queued mode. A queued
    /// binding with more pending samples than its share drops the oldest of them. In
    /// newest mode, nonzero too: the samples the binding keeps pending between steps; a
    /// step examines one.
    pub max_samples_per_iteration: usize,
    /// Nonzero excludes the binding from node health. A disabled input is never judged.
    pub optional: u8,
}

/// Time observed for one cycle iteration.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaCycleTime {
    /// The time the timer was scheduled to fire: simulated time under simulated time, else
    /// the monotonic host clock. Not a wall-clock stamp.
    pub scheduled_nanos: i64,
    /// Framework time at entry to the SDK callback, which every frame the step sends
    /// carries. A replay step under simulated time takes its deadline.
    pub callback_start_nanos: i64,
    /// Host monotonic time used to evaluate binding ages.
    pub capture_steady_nanos: i64,
    /// Time between the previous step and this one: the monotonic clock's difference on host
    /// time, the simulated clock's under simulated time.
    pub elapsed_nanos: i64,
    /// Periods that passed since the previous step without a step: 0 when the step is on
    /// time, in a replay step and in a triggered step.
    pub missed_periods: u64,
    /// Nonzero when elapsed time is present.
    pub has_elapsed: u8,
    /// Header stamp of this iteration, in nanoseconds since the Unix epoch.
    /// Framework time under simulated time, otherwise wall time.
    pub message_stamp_nanos: i64,
    /// Nonzero when a `RosTime` can hold the stamp. Zero also when the wall clock reads
    /// before the Unix epoch; the step still runs, so the caller checks the flag.
    pub message_stamp_valid: u8,
}

/// Borrowed payload selected for one binding during the cycle callback.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HyphaCycleSampleView {
    /// ROS wire payload bytes.
    pub payload: *const u8,
    /// Payload byte count.
    pub payload_len: usize,
    /// Host monotonic transport send time.
    pub transport_sent_nanos: u64,
    /// The sample's sequence in its source, for `hypha_cycle_image_reject_sample`.
    pub sequence: u64,
    /// The publisher instance that sent the frame.
    pub publisher_id: u64,
    /// The send sequence of that publisher, from the frame header. Wraps.
    pub publisher_sequence: u64,
}

/// The held sample facts of one input binding: the newest usable sample it captured.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaCycleHeldReport {
    /// Samples the source had accepted at capture. Wraps.
    pub received_total: u64,
    /// Usable samples that became the held sample. Wraps.
    pub delivered_total: u64,
    /// Nonzero when a held sample exists.
    pub has_held: u8,
    /// Sequence of the held sample. Zero when `has_held` is zero.
    pub held_sequence: u64,
}

/// Capture result for one input binding.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaCycleBindingReport {
    /// One of the `HYPHA_CYCLE_VERDICT_*` values.
    pub verdict: u32,
    /// Usable samples selected for this iteration.
    pub sample_count: usize,
    /// Queued retained samples still pending through the high-water mark. Zero for a
    /// latest binding.
    pub backlog_samples: usize,
    /// The oldest excess pending samples discarded in this step, and the samples a
    /// newest-only trigger passed for a newer one since its last step.
    pub work_limit_drops: u64,
    /// Samples overwritten before this binding examined them since last capture.
    pub lost_since_capture: u64,
    /// Cumulative samples lost to internal history retention for this binding.
    pub lost_total: u64,
    /// Samples older than the silence limit, and late triggers, in this capture.
    pub stale_since_capture: u64,
    /// Cumulative samples rejected by age for this binding.
    pub stale_total: u64,
}

/// Cumulative cycle outcomes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaCycleHealth {
    /// Completed step callbacks.
    pub iterations: u64,
    /// Cumulative timer missed periods.
    pub missed_periods: u64,
    /// Step and pre-dispatch failures.
    pub callback_failures: u64,
    /// Output send attempts.
    pub output_attempts: u64,
    /// Successful output sends.
    pub output_successes: u64,
    /// Failed output sends, including explicitly dropped pool pressure.
    pub output_failures: u64,
    /// Pool-exhausted outputs dropped under the diagnostic policy.
    pub output_pool_drops: u64,
    /// Framework time of the latest capture.
    pub observed_framework_nanos: i64,
    /// Host steady time of the latest capture.
    pub observed_steady_nanos: i64,
    /// Step callbacks that returned, failed ones included.
    pub steps: u64,
    /// Host monotonic duration of the last step callback; zero before the first.
    pub last_step_nanos: u64,
    /// The longest step callback since the commit, in host monotonic time; zero before
    /// the first.
    pub max_step_nanos: u64,
    /// Steps longer than the period, on host time only; zero for a triggered cycle and
    /// under simulated time.
    pub overruns: u64,
}

/// Cumulative outcomes for one input binding. Every count wraps.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaCycleBindingHealth {
    /// Most recent image report.
    pub capture: HyphaCycleBindingReport,
    /// Cumulative history losses and, for a queued binding, transport sequence gaps; a
    /// latest binding counts neither.
    pub overrun_total: u64,
    /// Cumulative samples intentionally discarded by queued work policies, newest-only
    /// passes included.
    pub work_limit_drops_total: u64,
    /// Cumulative age rejections.
    pub age_rejections_total: u64,
    /// Latest iterations with no usable value.
    pub latest_unavailable_iterations: u64,
    /// Transitions from usable to stale latest value.
    pub usable_to_stale_transitions: u64,
}

/// Everything one input binding reports: in a step from the image
/// (`hypha_cycle_image_binding`), or from the snapshot of the last finished step
/// (`hypha_cycle_binding`, from any thread).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaCycleBindingView {
    /// The cumulative counts, with the capture of the step the view describes.
    pub health: HyphaCycleBindingHealth,
    /// The held sample facts at that capture.
    pub held: HyphaCycleHeldReport,
    /// The supervision faults and the ignore flag at that capture. Zero for an
    /// unsupervised binding.
    pub supervision: HyphaSupervisionReport,
    /// 1 when the binding is ok for its node: unwired, advisory (`optional`), ignored,
    /// or usable and not stagnant.
    pub ok: u8,
    /// The age the verdict compared with the silence limit, on the capture clock: of
    /// the newest admitted sample, or, before any sample, the time since the first capture.
    pub age_nanos: u64,
}

/// The SDK's one call for a cycle iteration.
///
/// # Safety
/// Must not unwind. Image and emission views are valid only for this call.
pub type HyphaCycleStep = Option<
    extern "C" fn(
        image: *const HyphaCycleImage,
        time: *const HyphaCycleTime,
        emission: *mut HyphaCycleEmission,
        context: *mut c_void,
    ) -> crate::waitset::HyphaCallbackResult,
>;
