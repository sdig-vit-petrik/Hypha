//! Production cycle capture, emission validation, and output failure policy.

use std::fmt;
use std::sync::{Arc, Mutex};

mod arrivals;
mod helpers;
use helpers::{message_stamp_fits_ros_time, shares_history};

use super::{
    BindingCapture, BindingOptions, CaptureClock, CycleHistory, Supervision, SupervisionLimits,
    TriggerPorts, TriggerStep,
};

const STATUS_INVALID_ARGUMENT: i32 = 1;
const STATUS_POOL_EXHAUSTED: i32 = 3;
const STATUS_TOO_LARGE: i32 = 4;
const STATUS_BACKEND: i32 = 6;
const STATUS_RESOURCE_LIMIT: i32 = 13;
const NANOS_PER_SECOND: i64 = 1_000_000_000;

/// Timing of one cycle iteration.
///
///
/// The caller sets `scheduled_nanos`, `capture_steady_nanos`, `missed_periods`,
/// `replay_cut_nanos` and `trigger_step`; the runtime derives the rest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CycleTime {
    /// Framework time the timer was scheduled to fire. Set by the caller.
    pub scheduled_nanos: i64,
    /// Framework time at entry to the SDK callback. Derived.
    pub callback_start_nanos: i64,
    /// Host monotonic time used to evaluate binding ages. Set by the caller.
    pub capture_steady_nanos: i64,
    /// Time between the previous step and this one, on the monotonic clock on host time
    /// or the framework clock under simulated time. Derived.
    pub elapsed_nanos: i64,
    /// Periods that passed since the previous step without a step. Set by the caller.
    pub missed_periods: u64,
    /// True when a previous callback time exists. Derived.
    pub has_elapsed: bool,
    /// Header stamp in nanoseconds since the Unix epoch; zero without a stamp. Derived.
    pub message_stamp_nanos: i64,
    /// True when the message stamp is present and a `RosTime` can hold it. Derived.
    pub message_stamp_valid: bool,
    /// The data-time cut of a replay step, in framework nanoseconds: only samples stamped
    /// before it are admitted, or at it in a step of a triggered cycle, and ages count from
    /// it. `None` judges in host time.
    pub replay_cut_nanos: Option<u64>,
    /// What the trigger ports hold in a step of a triggered cycle; `None` for a rate cycle.
    /// Under a replay cut every input of such a step admits its samples through the cut.
    pub trigger_step: Option<TriggerStep>,
}

/// Cumulative cycle counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CycleHealth {
    /// Completed step callbacks.
    pub iterations: u64,
    /// Cumulative missed timer periods.
    pub missed_periods: u64,
    /// Step or pre-dispatch failures.
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
    /// Host monotonic duration of the last step callback. Zero before the first.
    pub last_step_nanos: u64,
    /// The longest step callback since the cycle was committed.
    pub max_step_nanos: u64,
    /// Steps longer than the cycle period, counted on host time only.
    pub overruns: u64,
}

/// Handling of one output publication refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFailurePolicy {
    /// Every publication refusal fails the owning `WaitSet`.
    FailRun,
    /// Only `PoolExhausted` is recorded and dropped; all other errors fail.
    DropPoolExhausted,
}

/// A publication result classified without depending on the C ABI status enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputError {
    /// Transport refused a loan because its pool is currently exhausted.
    PoolExhausted,
    /// Any other refusal, carried as the ABI status value.
    Failure(i32),
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PoolExhausted => {
                f.write_str("transport refused a loan because its pool is currently exhausted")
            }
            Self::Failure(status) => write!(f, "publication failed with status code {status}"),
        }
    }
}

impl std::error::Error for OutputError {}

/// One message a step registered, held until the step has finished.
///
/// `T` is whatever the caller stages the payload in: a byte buffer or a filled loan.
#[derive(Debug)]
pub struct StagedMessage<T> {
    /// The output the message goes to.
    pub output: u32,
    /// The payload length in bytes.
    pub len: usize,
    /// The staged payload, handed to the send callback in order.
    pub value: T,
}

struct BindingSlot {
    /// `None` for a disabled source, which has no history to read.
    history: Option<Arc<Mutex<CycleHistory>>>,
    options: BindingOptions,
    /// Takes its own arrivals limit outside the cycle's arrival budget.
    exempt: bool,
}

/// Owns the production capture and output-policy state for one cycle.
///
/// The FFI adapter and the deterministic Rust driver share
/// [`CycleRuntime::run_iteration_with_emissions`].
pub struct CycleRuntime {
    bindings: Vec<BindingSlot>,
    captures: Vec<BindingCapture>,
    outputs: Vec<OutputBinding>,
    output_counts: Vec<u32>,
    arrival_budget: Option<usize>,
    arrival_rotation: usize,
    /// Per-tick scratch for the arrival budget; reused, so a steady iteration allocates nothing.
    capacities: Vec<usize>,
    assignments: Vec<usize>,
    active: Vec<usize>,
    /// The framework time of the previous step's entry, for `elapsed_nanos` under simulated time.
    last_callback_start: Option<i64>,
    /// The monotonic capture time of the previous step, for `elapsed_nanos` on host time.
    last_capture_steady: Option<i64>,
    health: CycleHealth,
    /// The trigger bindings of a triggered cycle, in binding order, outside the arrival
    /// budget. A trigger port is named by its position here.
    triggers: Vec<usize>,
}

struct OutputBinding {
    policy: OutputFailurePolicy,
    max_payload_bytes: usize,
    max_messages_per_iteration: u32,
}

/// The output limits of one step, which admits each message as the step stages it.
///
///
/// A message over its output's count gets `STATUS_RESOURCE_LIMIT`, one over its
/// payload limit `STATUS_TOO_LARGE`, and an unknown output `STATUS_INVALID_ARGUMENT`.
pub struct EmissionBudget<'a> {
    outputs: &'a [OutputBinding],
    counts: &'a mut [u32],
}

impl<'a> EmissionBudget<'a> {
    fn new(outputs: &'a [OutputBinding], counts: &'a mut Vec<u32>) -> Self {
        counts.resize(outputs.len(), 0);
        counts.fill(0);
        Self { outputs, counts }
    }

    /// Count one message of `len` payload bytes on `output`.
    ///
    /// # Errors
    /// The status code of the limit the message breaks, as documented on the type.
    pub fn admit(&mut self, output: u32, len: usize) -> Result<(), i32> {
        let index = usize::try_from(output).map_err(|_| STATUS_INVALID_ARGUMENT)?;
        let limits = self.outputs.get(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        let count = self.counts.get_mut(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        if *count >= limits.max_messages_per_iteration {
            return Err(STATUS_RESOURCE_LIMIT);
        }
        if len > limits.max_payload_bytes {
            return Err(STATUS_TOO_LARGE);
        }
        *count = count.checked_add(1).ok_or(STATUS_RESOURCE_LIMIT)?;
        Ok(())
    }

    /// Give back the count of one message of `output` that [`EmissionBudget::admit`]
    /// admitted and the step will not send.
    pub fn refund(&mut self, output: u32) {
        let count = usize::try_from(output).ok().and_then(|index| self.counts.get_mut(index));
        if let Some(count) = count {
            *count = count.saturating_sub(1);
        }
    }
}

impl CycleRuntime {
    /// Create an empty runtime.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bindings: Vec::new(),
            captures: Vec::new(),
            outputs: Vec::new(),
            output_counts: Vec::new(),
            arrival_budget: None,
            arrival_rotation: 0,
            capacities: Vec::new(),
            assignments: Vec::new(),
            active: Vec::new(),
            last_callback_start: None,
            last_capture_steady: None,
            health: CycleHealth {
                iterations: 0,
                missed_periods: 0,
                callback_failures: 0,
                output_attempts: 0,
                output_successes: 0,
                output_failures: 0,
                output_pool_drops: 0,
                observed_framework_nanos: 0,
                observed_steady_nanos: 0,
                steps: 0,
                last_step_nanos: 0,
                max_step_nanos: 0,
                overruns: 0,
            },
            triggers: Vec::new(),
        }
    }

    /// Set the cycle-wide queued-sample budget. `None` leaves only per-binding limits.
    ///
    /// # Errors
    ///
    /// Returns `STATUS_INVALID_ARGUMENT` when the supplied budget is zero.
    pub fn set_arrival_budget(&mut self, budget: Option<usize>) -> Result<(), i32> {
        if budget == Some(0) {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        self.arrival_budget = budget;
        Ok(())
    }

    /// Add binding `index` to the triggers of a triggered cycle; a repeat adds nothing.
    /// Each step takes the samples its [`CycleTime::trigger_step`] names, outside the
    /// binding's limits and the arrival budget.
    ///
    /// # Errors
    /// Returns `STATUS_INVALID_ARGUMENT` unless the binding exists, is queued and has a
    /// source, and no other trigger reads its history, where each sample would step
    /// twice; `STATUS_RESOURCE_LIMIT` past [`TriggerPorts::CAPACITY`] triggers.
    pub fn set_trigger(&mut self, index: usize) -> Result<(), i32> {
        let slot = self.bindings.get(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        if slot.options.mode != super::CaptureMode::Queued || slot.history.is_none() {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        let Err(position) = self.triggers.binary_search(&index) else {
            return Ok(());
        };
        let shared = self
            .triggers
            .iter()
            .filter_map(|&trigger| self.bindings.get(trigger))
            .any(|trigger| shares_history(trigger, slot));
        if shared {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        if self.triggers.len() >= TriggerPorts::CAPACITY {
            return Err(STATUS_RESOURCE_LIMIT);
        }
        self.triggers.insert(position, index);
        if let Some(capture) = self.captures.get_mut(index) {
            capture.set_trigger();
        }
        Ok(())
    }

    /// Capture latest binding `index` on the host clock, also in a replay step: see
    /// [`BindingCapture::host_clock`].
    ///
    /// # Errors
    /// Returns `STATUS_INVALID_ARGUMENT` unless the binding exists and is latest.
    pub fn set_host_clock(&mut self, index: usize) -> Result<(), i32> {
        let slot = self.bindings.get(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        if slot.options.mode != super::CaptureMode::Latest {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        self.captures.get_mut(index).ok_or(STATUS_INVALID_ARGUMENT)?.set_host_clock();
        Ok(())
    }

    /// Let queued binding `index` take up to its own arrivals limit outside the arrival
    /// budget. A TF input needs every transform.
    ///
    /// # Errors
    /// Returns `STATUS_INVALID_ARGUMENT` unless the binding exists and is queued.
    pub fn exempt_from_budget(&mut self, index: usize) -> Result<(), i32> {
        let slot = self.bindings.get_mut(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        if slot.options.mode != super::CaptureMode::Queued {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        slot.exempt = true;
        Ok(())
    }

    /// The trigger bindings, in binding order; empty for a rate cycle. Trigger port `p`
    /// is binding `triggers()[p]`.
    #[must_use]
    pub fn triggers(&self) -> &[usize] {
        &self.triggers
    }

    /// Replace `out` with one list per trigger port: the framework send stamps of the
    /// samples it has not taken, in history order. Empty for a rate cycle.
    ///
    /// # Errors
    /// Returns `STATUS_INVALID_ARGUMENT` for a stamp beyond `i64::MAX`.
    pub fn pending_trigger_stamps(&self, out: &mut Vec<Vec<i64>>) -> Result<(), i32> {
        out.resize_with(self.triggers.len(), Vec::new);
        for (&index, stamps) in self.triggers.iter().zip(out.iter_mut()) {
            stamps.clear();
            let history = self.bindings.get(index).and_then(|slot| slot.history.as_ref());
            let (Some(history), Some(capture)) = (history, self.captures.get(index)) else {
                return Err(STATUS_INVALID_ARGUMENT);
            };
            let history = crate::locked(history);
            for stamp in capture.pending_stamps(&history) {
                stamps.push(i64::try_from(stamp).map_err(|_| STATUS_INVALID_ARGUMENT)?);
            }
        }
        Ok(())
    }

    /// Add a live source binding, starting its cursor at current retained data.
    ///
    /// # Errors
    ///
    /// Returns an error status code if the capture cannot be created, or the maximum
    /// number of bindings is exceeded.
    pub fn add_binding(
        &mut self,
        history: Arc<Mutex<CycleHistory>>,
        options: BindingOptions,
    ) -> Result<u32, i32> {
        let held = crate::locked(&history);
        let capture = BindingCapture::new(&held, options.mode, options.max_samples_per_iteration)
            .ok_or(STATUS_INVALID_ARGUMENT)?;
        drop(held);
        self.push_binding(Some(history), capture, options)
    }

    /// Add an optional source that was not wired; it never receives a sample.
    ///
    /// # Errors
    ///
    /// Returns an error status code if the binding options are invalid or the
    /// maximum number of bindings is exceeded.
    pub fn add_disabled_binding(&mut self, options: BindingOptions) -> Result<u32, i32> {
        let capture = BindingCapture::absent(options.mode, options.max_samples_per_iteration)
            .ok_or(STATUS_INVALID_ARGUMENT)?;
        self.push_binding(None, capture, options)
    }

    fn push_binding(
        &mut self,
        history: Option<Arc<Mutex<CycleHistory>>>,
        capture: BindingCapture,
        options: BindingOptions,
    ) -> Result<u32, i32> {
        let index = u32::try_from(self.bindings.len()).map_err(|_| STATUS_RESOURCE_LIMIT)?;
        self.bindings.push(BindingSlot { history, options, exempt: false });
        self.captures.push(capture);
        Ok(index)
    }

    /// Add an output policy and return its declaration-order index.
    ///
    /// # Errors
    ///
    /// Returns an error status code if the maximum number of outputs is exceeded.
    pub fn add_output(
        &mut self,
        policy: OutputFailurePolicy,
        max_payload_bytes: usize,
        max_messages_per_iteration: u32,
    ) -> Result<u32, i32> {
        if max_messages_per_iteration == 0 {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        let index = u32::try_from(self.outputs.len()).map_err(|_| STATUS_RESOURCE_LIMIT)?;
        self.outputs.push(OutputBinding { policy, max_payload_bytes, max_messages_per_iteration });
        Ok(index)
    }

    /// Capture inputs, read the clock through `callback_start` (at most once,
    /// after capture), set the message stamp, invoke `step`, and send.
    /// `emissions` is caller-owned staging. A failed step or validation drops every staged
    /// value unsent, so a loan in `T` is released; otherwise `send` receives them in order.
    ///
    /// # Errors
    ///
    /// Returns the status that failed the iteration: capture, step, emission
    /// validation, or the first output send the policy does not drop.
    pub fn run_iteration_with_emissions<ReadClock, S, D, T>(
        &mut self,
        mut time: CycleTime,
        callback_start: ReadClock,
        message_stamp: Option<i64>,
        emissions: &mut Vec<StagedMessage<T>>,
        step: S,
        send: D,
    ) -> Result<(), i32>
    where
        ReadClock: FnOnce() -> i64,
        S: FnOnce(
            &[BindingCapture],
            CycleTime,
            &mut Vec<StagedMessage<T>>,
            &mut EmissionBudget<'_>,
        ) -> Result<(), i32>,
        D: FnMut(u32, T) -> Result<(), OutputError>,
    {
        let Ok(capture_steady_u64) = u64::try_from(time.capture_steady_nanos) else {
            return self.fail(STATUS_INVALID_ARGUMENT);
        };
        let clock = match (time.replay_cut_nanos, time.trigger_step) {
            // A triggered step at T reads every input through T: the samples one upstream
            // step sent with the trigger sample carry T, and the upstream hold has every
            // sample stamped T of the reporting upstreams queued (§6.6, #231).
            (Some(cut_nanos), Some(_)) => CaptureClock::ReplayThrough { cut_nanos },
            (Some(cut_nanos), None) => CaptureClock::Replay { cut_nanos },
            (None, _) => CaptureClock::Host { steady_nanos: capture_steady_u64 },
        };
        let sent_through = self.firing_sent_nanos(clock, time);
        if let Err(status) = self.arrival_assignments(clock, sent_through) {
            return self.fail(status);
        }
        if let Err(status) = self.capture_bindings(clock, sent_through, time) {
            return self.fail(status);
        }
        let callback_start_nanos = callback_start();
        // Host time measures the gap on the monotonic clock, so a wall step cannot make it negative.
        let host_step = time.replay_cut_nanos.is_none();
        let elapsed = if host_step {
            self.last_capture_steady
                .and_then(|previous| time.capture_steady_nanos.checked_sub(previous))
        } else {
            self.last_callback_start.and_then(|previous| callback_start_nanos.checked_sub(previous))
        };
        self.last_callback_start = Some(callback_start_nanos);
        self.last_capture_steady = Some(time.capture_steady_nanos);
        self.health.observed_framework_nanos = callback_start_nanos;
        self.health.observed_steady_nanos = time.capture_steady_nanos;
        self.health.missed_periods = self.health.missed_periods.wrapping_add(time.missed_periods);
        time.callback_start_nanos = callback_start_nanos;
        time.elapsed_nanos = elapsed.unwrap_or(0);
        time.has_elapsed = elapsed.is_some();
        time.message_stamp_nanos = message_stamp.unwrap_or(0);
        time.message_stamp_valid = message_stamp.is_some_and(message_stamp_fits_ros_time);

        emissions.clear();
        let mut budget = EmissionBudget::new(&self.outputs, &mut self.output_counts);
        // A failed step drops what it staged unsent, so no staged loan outlives it.
        if let Err(status) = step(&self.captures, time, emissions, &mut budget) {
            emissions.clear();
            return self.fail(status);
        }

        self.dispatch_emissions(emissions, send)
    }

    fn dispatch_emissions<T, D>(
        &mut self,
        emissions: &mut Vec<StagedMessage<T>>,
        mut send: D,
    ) -> Result<(), i32>
    where
        D: FnMut(u32, T) -> Result<(), OutputError>,
    {
        if let Err(status) = self.validate_emissions(emissions) {
            emissions.clear();
            return self.fail(status);
        }

        self.health.iterations = self.health.iterations.wrapping_add(1);
        for staged in emissions.drain(..) {
            let index = staged.output;
            self.health.output_attempts = self.health.output_attempts.wrapping_add(1);
            let Err(error) = send(index, staged.value) else {
                self.health.output_successes = self.health.output_successes.wrapping_add(1);
                continue;
            };
            self.health.output_failures = self.health.output_failures.wrapping_add(1);
            let drops_pool_exhausted = error == OutputError::PoolExhausted
                && self
                    .outputs
                    .get(usize::try_from(index).unwrap_or(usize::MAX))
                    .is_some_and(|output| output.policy == OutputFailurePolicy::DropPoolExhausted);
            if drops_pool_exhausted {
                self.health.output_pool_drops = self.health.output_pool_drops.wrapping_add(1);
                continue;
            }
            return Self::failure(match error {
                OutputError::PoolExhausted => STATUS_POOL_EXHAUSTED,
                OutputError::Failure(status) => status,
            });
        }
        Ok(())
    }

    /// Check every staged message against its output's count and payload limit.
    fn validate_emissions<T>(&mut self, emissions: &[StagedMessage<T>]) -> Result<(), i32> {
        let mut budget = EmissionBudget::new(&self.outputs, &mut self.output_counts);
        emissions.iter().try_for_each(|staged| budget.admit(staged.output, staged.len))
    }

    /// Host monotonic send time of the sample a trigger step on host time fires on; `None`
    /// for a fallback, a rate step and a replay step.
    ///
    /// It bounds what the step's queued inputs that are no trigger take: the samples sent
    /// up to it. One sent after it belongs to a later step, so the trigger samples drained
    /// in one turn each find their own arrivals, as they do under a replay cut. A sample
    /// sent with the firing one, by one step of one upstream cycle, carries the same send
    /// time and is taken.
    fn firing_sent_nanos(&self, clock: CaptureClock, time: CycleTime) -> Option<u64> {
        let (CaptureClock::Host { .. }, Some(TriggerStep::Sample { port })) =
            (clock, time.trigger_step)
        else {
            return None;
        };
        let index = *self.triggers.get(port)?;
        let history = self.bindings.get(index)?.history.as_ref()?;
        self.captures.get(index)?.next_transport_sent_nanos(&crate::locked(history))
    }

    /// Capture every binding. A shared history is locked once, so all read one snapshot.
    /// With `sent_through`, the queued inputs that are no trigger take the samples sent up
    /// to it ([`Self::firing_sent_nanos`]).
    fn capture_bindings(
        &mut self,
        clock: CaptureClock,
        sent_through: Option<u64>,
        time: CycleTime,
    ) -> Result<(), i32> {
        for (first, slot) in self.bindings.iter().enumerate() {
            let Some(history) = slot.history.as_ref() else {
                if let Some(capture) = self.captures.get_mut(first) {
                    capture.capture_absent(slot.options, clock);
                    capture.supervise(time.scheduled_nanos);
                }
                continue;
            };
            if self.bindings.iter().take(first).any(|earlier| shares_history(earlier, slot)) {
                continue;
            }
            let guard = crate::locked(history);
            for (index, (other, capture)) in
                self.bindings.iter().zip(&mut self.captures).enumerate().skip(first)
            {
                if !shares_history(slot, other) {
                    continue;
                }
                if let Some(port) = self.triggers.iter().position(|&trigger| trigger == index) {
                    let step = time.trigger_step.ok_or(STATUS_INVALID_ARGUMENT)?;
                    let options = other.options;
                    capture.capture_trigger(&guard, options, step.fires(port), clock);
                    // The schedule found the port silent; the capture clock may read a
                    // little earlier than the schedule's.
                    if step.is_due(port) {
                        capture.mark_stale(options.silence_nanos);
                    }
                } else if other.options.mode == super::CaptureMode::Queued {
                    let assigned = self.assignments.get(index).copied().unwrap_or_default();
                    // The transform input keeps every arrival: a lookup wants the newest.
                    let sent_through = sent_through.filter(|_| !other.exempt);
                    capture.capture_arrivals(&guard, other.options, assigned, clock, sent_through);
                } else if capture.host_clock() {
                    let steady_nanos = u64::try_from(time.capture_steady_nanos)
                        .map_err(|_| STATUS_INVALID_ARGUMENT)?;
                    capture.capture_on(&guard, other.options, CaptureClock::Host { steady_nanos });
                } else {
                    capture.capture_on(&guard, other.options, clock);
                }
                capture.supervise(time.scheduled_nanos);
            }
        }
        Ok(())
    }

    /// Supervise one binding with `limits`, replacing any earlier state.
    ///
    /// # Errors
    ///
    /// Returns `STATUS_INVALID_ARGUMENT` if `index` names no binding.
    pub fn set_supervision(
        &mut self,
        index: usize,
        limits: SupervisionLimits,
        ignored: bool,
    ) -> Result<(), i32> {
        let slot = self.bindings.get_mut(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        slot.options.silence_nanos = limits.silence_nanos;
        let mut supervision = Supervision::new(limits);
        supervision.set_ignored(ignored);
        let capture = self.captures.get_mut(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        capture.set_supervision(Some(supervision));
        Ok(())
    }

    /// Start the silence count of every trigger port at `clock_nanos` on its capture
    /// clock, unless a capture started it: the first evaluation of a triggered cycle.
    pub fn start_trigger_silence(&mut self, clock_nanos: u64) {
        for &index in &self.triggers {
            if let Some(capture) = self.captures.get_mut(index) {
                capture.start_at(clock_nanos);
            }
        }
    }

    /// Consume the oldest pending sample of trigger port `port` without a step, and count
    /// it as an age rejection of that port.
    ///
    /// # Errors
    ///
    /// Returns `STATUS_INVALID_ARGUMENT` without such a port or a pending sample.
    pub fn refuse_trigger(&mut self, port: usize) -> Result<(), i32> {
        let index = *self.triggers.get(port).ok_or(STATUS_INVALID_ARGUMENT)?;
        let history = self.bindings.get(index).and_then(|slot| slot.history.as_ref());
        let (Some(history), Some(capture)) = (history, self.captures.get_mut(index)) else {
            return Err(STATUS_INVALID_ARGUMENT);
        };
        let history = crate::locked(history);
        if capture.refuse_pending(&history) { Ok(()) } else { Err(STATUS_INVALID_ARGUMENT) }
    }

    /// Consume the oldest pending sample of trigger port `port` without a step: a newer
    /// sample of the port supersedes it. It counts as a work-limit drop of that port.
    ///
    /// # Errors
    ///
    /// Returns `STATUS_INVALID_ARGUMENT` without such a port or a pending sample.
    pub fn supersede_trigger(&mut self, port: usize) -> Result<(), i32> {
        let index = *self.triggers.get(port).ok_or(STATUS_INVALID_ARGUMENT)?;
        let history = self.bindings.get(index).and_then(|slot| slot.history.as_ref());
        let (Some(history), Some(capture)) = (history, self.captures.get_mut(index)) else {
            return Err(STATUS_INVALID_ARGUMENT);
        };
        let history = crate::locked(history);
        if capture.supersede_pending(&history) { Ok(()) } else { Err(STATUS_INVALID_ARGUMENT) }
    }

    /// Report the alive counter of a binding's newest accepted sample.
    /// Ignored when the index is unknown or the binding is not supervised.
    pub fn apply_alive(&mut self, index: usize, value: u64) {
        if let Some(supervision) = self.supervision_mut(index) {
            supervision.on_alive(value);
        }
    }

    /// Override or restore the fault of a binding.
    /// Ignored when the index is unknown or the binding is not supervised.
    pub fn set_ignored(&mut self, index: usize, ignored: bool) {
        if let Some(supervision) = self.supervision_mut(index) {
            supervision.set_ignored(ignored);
        }
    }

    /// The judgement changes of a binding since the last call, in order.
    pub fn take_judgement_events(&mut self, index: usize) -> [Option<crate::JudgementEvent>; 2] {
        let silence_nanos = self.bindings.get(index).map_or(0, |slot| slot.options.silence_nanos);
        let Some(capture) = self.captures.get_mut(index) else {
            return [None, None];
        };
        capture.take_judgement_events(silence_nanos)
    }

    /// Record one step callback that returned after `duration_nanos` of host time. It
    /// is an overrun when `period_nanos` is nonzero and the step took longer.
    pub fn record_step(&mut self, duration_nanos: u64, period_nanos: u64) {
        self.health.steps = self.health.steps.wrapping_add(1);
        self.health.last_step_nanos = duration_nanos;
        self.health.max_step_nanos = self.health.max_step_nanos.max(duration_nanos);
        let overran = period_nanos != 0 && duration_nanos > period_nanos;
        if overran {
            self.health.overruns = self.health.overruns.wrapping_add(1);
        }
    }

    /// Clear binding `index` after the step that captured it last: see
    /// [`BindingCapture::clear`]. Returns the pending samples it dropped.
    ///
    /// # Errors
    /// Returns `STATUS_INVALID_ARGUMENT` if `index` names no binding, or a trigger port,
    /// whose pending samples are the steps the schedule has planned.
    pub fn clear_binding(&mut self, index: usize) -> Result<u64, i32> {
        let slot = self.bindings.get(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        let capture = self.captures.get_mut(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        if capture.is_trigger() {
            return Err(STATUS_INVALID_ARGUMENT);
        }
        let history = slot.history.as_ref().map(|history| crate::locked(history));
        Ok(capture.clear(history.as_deref()))
    }

    /// Re-judge binding `index` after the step rejected its sample `sequence` as
    /// undecodable. Ignored for an unknown index or a binding without a source.
    pub fn reject_sample(&mut self, index: usize, sequence: u64) {
        let Some(history) = self.bindings.get(index).and_then(|slot| slot.history.as_ref()) else {
            return;
        };
        let Some(capture) = self.captures.get_mut(index) else {
            return;
        };
        let history = crate::locked(history);
        capture.reject(&history, sequence);
    }

    /// Current cycle diagnostics.
    #[must_use]
    pub const fn health(&self) -> CycleHealth {
        self.health
    }

    /// A binding's frozen capture counters.
    #[must_use]
    pub fn binding_capture(&self, index: usize) -> Option<&BindingCapture> {
        self.captures.get(index)
    }

    fn supervision_mut(&mut self, index: usize) -> Option<&mut Supervision> {
        self.captures.get_mut(index)?.supervision_mut()
    }

    fn fail(&mut self, status: i32) -> Result<(), i32> {
        self.health.callback_failures = self.health.callback_failures.wrapping_add(1);
        Self::failure(status)
    }

    fn failure(status: i32) -> Result<(), i32> {
        Err(if status == 0 { STATUS_BACKEND } else { status })
    }
}

impl Default for CycleRuntime {
    fn default() -> Self {
        Self::new()
    }
}
