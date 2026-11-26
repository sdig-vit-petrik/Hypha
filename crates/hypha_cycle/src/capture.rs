//! Deterministic per-binding capture over shared history.

use std::sync::Arc;

mod clock;
use clock::{accepted_on, value_usable_within};

use super::{CycleHistory, HistorySample, Supervision};

/// Whether a binding selects state or consumes a bounded queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// Select the newest retained sample through the capture high-water mark.
    Latest,
    /// Consume each retained sample once through an independent cursor.
    Queued,
}

/// Action when a queued binding has more pending samples than its assigned work.
///
/// `Carry` is the capture of a cycle's trigger ports; a C ABI binding always drops.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueueOverflowPolicy {
    /// Discard oldest excess samples, then examine newest assigned samples; the only
    /// policy of a latest binding.
    #[default]
    DropOldest,
    /// Keep unexamined samples for a later iteration.
    Carry,
}

/// The clock a capture admits and judges its samples on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureClock {
    /// Host monotonic time. Every retained sample is admitted and judged by its send time.
    Host {
        /// Host monotonic time of the capture.
        steady_nanos: u64,
    },
    /// Data time for one replay step: only samples sent before `cut_nanos` are admitted,
    /// each judged by `cut_nanos - framework_sent_nanos`.
    Replay {
        /// The deadline of the step, in framework nanoseconds.
        cut_nanos: u64,
    },
    /// Data time for a trigger port of a replay step: the samples sent at or before
    /// `cut_nanos` are admitted, each judged by `cut_nanos - framework_sent_nanos`.
    ReplayThrough {
        /// The time of the step, in framework nanoseconds.
        cut_nanos: u64,
    },
}

/// One binding's capture policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingOptions {
    /// Latest state or queued samples.
    pub mode: CaptureMode,
    /// The silence limit in nanoseconds: past it a sample is stale. Zero disables the check.
    pub silence_nanos: u64,
    /// Maximum queued samples examined in one capture. Zero is invalid.
    pub max_samples_per_iteration: usize,
    /// Action when pending samples exceed the work assigned to this binding.
    pub overflow_policy: QueueOverflowPolicy,
}

/// Freshness of a captured binding: one judgement of its newest admitted sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputVerdict {
    /// The capture clock has admitted no sample since the binding was created.
    NeverArrived,
    /// The newest admitted sample (for a latest binding with none retained, the held
    /// one) is within the silence limit.
    Usable,
    /// The newest admitted sample is older than the silence limit, or stamped after the
    /// capture clock, or none is left to judge.
    Stale,
}

/// Cumulative per-binding diagnostic counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BindingCounters {
    /// Samples examined but no longer retained when this binding caught up.
    pub overrun_total: u64,
    /// Samples intentionally discarded by a queue overflow policy.
    pub work_limit_drops_total: u64,
    /// Samples rejected by this binding's silence limit, and late triggers.
    pub age_rejections_total: u64,
    /// Latest captures that had no usable value.
    pub latest_unavailable_iterations: u64,
    /// Transitions from usable latest value to stale value.
    pub usable_to_stale_transitions: u64,
}

/// The immutable samples and diagnostics frozen for one binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingCapture {
    /// Samples usable under this binding's age rule.
    pub samples: Vec<Arc<HistorySample>>,
    /// Verdict for this capture.
    pub verdict: InputVerdict,
    /// Queued retained samples not examined because the work limit was reached.
    pub backlog_samples: usize,
    /// Samples intentionally discarded in this capture by the overflow policy.
    pub work_limit_drops: u64,
    /// Internal-history losses since this binding last captured.
    pub lost_since_capture: u64,
    /// Age rejections in this capture.
    pub stale_since_capture: u64,
    /// Per-binding cumulative counters after this capture.
    pub counters: BindingCounters,
    /// The age the verdict compared with the silence limit, on the capture clock.
    pub age_nanos: u64,
    held: Option<Arc<HistorySample>>,
    /// The held sample before the last one, restored when a reader rejects the last.
    previous_held: Option<Arc<HistorySample>>,
    /// The clock and silence limit of the last judgement, for a re-judgement.
    last_judged: Option<(CaptureClock, u64)>,
    mode: CaptureMode,
    received_total: u64,
    delivered_total: u64,
    cursor: u64,
    previous_latest_verdict: Option<InputVerdict>,
    supervision: Option<Supervision>,
    /// The capture clock at the first capture or evaluation; silence counts from it.
    first_nanos: Option<u64>,
    stale: bool,
    /// Captured on the host clock under replay too: see [`BindingCapture::host_clock`].
    host_clock: bool,
    /// The judgement last reported by [`BindingCapture::take_judgement_events`].
    reported: crate::judgement::JudgementTracker,
}

impl BindingCapture {
    /// Start a binding at the oldest currently retained sample.
    #[must_use]
    pub fn new(
        history: &CycleHistory,
        mode: CaptureMode,
        max_samples_per_iteration: usize,
    ) -> Option<Self> {
        let cursor =
            history.samples().front().map_or(history.high_water_next(), |sample| sample.sequence);
        Self::with_cursor(mode, max_samples_per_iteration, cursor)
    }

    /// Start a binding with no source. It never receives a sample.
    #[must_use]
    pub fn absent(mode: CaptureMode, max_samples_per_iteration: usize) -> Option<Self> {
        Self::with_cursor(mode, max_samples_per_iteration, 0)
    }

    fn with_cursor(
        mode: CaptureMode,
        max_samples_per_iteration: usize,
        cursor: u64,
    ) -> Option<Self> {
        let queued_binding_has_no_sample_budget =
            mode == CaptureMode::Queued && max_samples_per_iteration == 0;
        if queued_binding_has_no_sample_budget {
            return None;
        }
        Some(Self {
            samples: Vec::new(),
            verdict: InputVerdict::NeverArrived,
            backlog_samples: 0,
            work_limit_drops: 0,
            lost_since_capture: 0,
            stale_since_capture: 0,
            counters: BindingCounters::default(),
            age_nanos: 0,
            held: None,
            previous_held: None,
            last_judged: None,
            mode,
            received_total: 0,
            delivered_total: 0,
            cursor,
            previous_latest_verdict: None,
            supervision: None,
            first_nanos: None,
            stale: false,
            host_clock: false,
            reported: crate::judgement::JudgementTracker::default(),
        })
    }

    /// The newest usable sample this binding has captured. A stale sample never
    /// becomes the held sample, and the held sample survives history eviction.
    #[must_use]
    pub const fn held(&self) -> Option<&Arc<HistorySample>> {
        self.held.as_ref()
    }

    /// Whether this binding selects the newest value or consumes a queue.
    #[must_use]
    pub const fn mode(&self) -> CaptureMode {
        self.mode
    }

    /// Whether this latest binding reads data stamped on another clock; a replay step
    /// then captures it as a host-time step does.
    #[must_use]
    pub const fn host_clock(&self) -> bool {
        self.host_clock
    }

    pub(crate) const fn set_host_clock(&mut self) {
        self.host_clock = true;
    }

    /// Supervision state, or `None` when this binding is not supervised.
    #[must_use]
    pub const fn supervision(&self) -> Option<&Supervision> {
        self.supervision.as_ref()
    }

    /// Mutable supervision state, or `None` when this binding is not supervised.
    pub(super) fn supervision_mut(&mut self) -> Option<&mut Supervision> {
        self.supervision.as_mut()
    }

    /// Attach or detach supervision. Only the cycle runtime calls this.
    pub(super) fn set_supervision(&mut self, supervision: Option<Supervision>) {
        self.supervision = supervision;
    }

    /// Run the supervision checks at framework time `framework_now_nanos`.
    /// Call after `capture`. A binding that is not supervised does nothing.
    pub fn supervise(&mut self, framework_now_nanos: i64) {
        let Some(supervision) = self.supervision.as_mut() else {
            return;
        };
        supervision.on_capture(framework_now_nanos, self.received_total, self.stale);
    }

    /// Whether the input was stale at the last capture. Always false without a limit.
    #[must_use]
    pub const fn stale(&self) -> bool {
        self.stale
    }

    /// Whether this binding is ok for its node: always for an unwired, advisory or
    /// ignored binding, else while usable and not stagnant.
    #[must_use]
    pub fn ok(&self, counts: bool, supervision: Option<&Supervision>) -> bool {
        let ignored = supervision.is_some_and(Supervision::ignored);
        let stagnant = supervision
            .is_some_and(|state| state.faults() & crate::supervision::FAULT_STAGNANT != 0);
        let fresh_and_alive = self.verdict == InputVerdict::Usable && !stagnant;
        !counts || ignored || fresh_and_alive
    }

    /// The changes of this binding's judgement since the last call: its condition, then
    /// its ignore flag while faulted.
    pub fn take_judgement_events(
        &mut self,
        silence_nanos: u64,
    ) -> [Option<crate::JudgementEvent>; 2] {
        let mut reported = self.reported;
        let events = reported.observe(self, silence_nanos);
        self.reported = reported;
        events
    }

    /// Re-judge this capture after a reader rejected the sample `sequence` as
    /// undecodable ([`CycleHistory::reject`], already applied to `history`): the verdict,
    /// fault and age are judged again on the same clock. The captured samples stay as
    /// they are, so the step's indices hold.
    pub fn reject(&mut self, history: &CycleHistory, sequence: u64) {
        let held_rejected = self.held.as_ref().is_some_and(|held| held.sequence == sequence);
        let in_batch = self.samples.iter().any(|sample| sample.sequence == sequence);
        let queued_delivery = self.mode == CaptureMode::Queued && in_batch;
        let latest_delivery = self.mode == CaptureMode::Latest && held_rejected;
        if queued_delivery || latest_delivery {
            self.delivered_total = self.delivered_total.wrapping_sub(1);
        }
        if latest_delivery {
            self.held = self.previous_held.take();
        }
        let Some((clock, silence_nanos)) = self.last_judged else {
            return;
        };
        self.received_total = accepted_on(history, clock);
        self.judge(Some(history), silence_nanos, clock);
        let usable_newer = latest_delivery && self.verdict == InputVerdict::Usable;
        if usable_newer && let Some(newest) = clock.newest_admitted_arc(history) {
            self.hold_latest_usable(newest);
        }
        if let Some(supervision) = self.supervision.as_mut() {
            supervision.set_stale(self.stale);
        }
    }

    /// Mark this capture stale when `silence_nanos` is a limit: a trigger port the
    /// fallback step of a triggered cycle found silent. An input that never arrived stays
    /// `NeverArrived`.
    pub(crate) fn mark_stale(&mut self, silence_nanos: u64) {
        if silence_nanos == 0 {
            return;
        }
        self.stale = true;
        if self.verdict == InputVerdict::Usable {
            self.verdict = InputVerdict::Stale;
        }
    }

    /// Count the silence from `clock_nanos`, on the capture clock, unless a capture or
    /// an earlier call started it.
    pub fn start_at(&mut self, clock_nanos: u64) {
        self.first_nanos.get_or_insert(clock_nanos);
    }

    /// The one freshness judgement of this binding at `clock`. Call after
    /// `received_total` is set and before the samples are selected.
    fn judge(&mut self, history: Option<&CycleHistory>, silence_nanos: u64, clock: CaptureClock) {
        self.last_judged = Some((clock, silence_nanos));
        let now = clock.nanos();
        let first = *self.first_nanos.get_or_insert(now);
        if self.received_total == 0 {
            self.age_nanos = now.saturating_sub(first);
            self.stale = silence_nanos != 0 && self.age_nanos > silence_nanos;
            self.verdict = InputVerdict::NeverArrived;
            return;
        }
        let newest_admitted = history.and_then(|history| clock.newest_admitted(history));
        let held = self.held.as_deref().filter(|_| self.mode == CaptureMode::Latest);
        let reference = newest_admitted.or(held);
        let fresh = if let Some(sample) = reference {
            let sent = clock.sent(sample);
            self.age_nanos = now.saturating_sub(sent);
            value_usable_within(silence_nanos, sent, now)
        } else {
            self.age_nanos = now.saturating_sub(first);
            false
        };
        self.verdict = if fresh { InputVerdict::Usable } else { InputVerdict::Stale };
        self.stale = !fresh;
    }

    /// Advance past the oldest pending sample without delivering it, and count it as
    /// rejected. Returns whether a pending sample existed.
    pub fn refuse_pending(&mut self, history: &CycleHistory) -> bool {
        let start = history.cursor_offset(self.cursor);
        let Some(sample) = history.samples().get(start) else {
            return false;
        };
        self.cursor = sample.sequence.wrapping_add(1);
        self.counters.age_rejections_total = self.counters.age_rejections_total.wrapping_add(1);
        true
    }

    /// Usable samples this binding has counted, each one once. Wraps.
    #[must_use]
    pub const fn delivered_total(&self) -> u64 {
        self.delivered_total
    }

    /// Samples the history had accepted at the last capture. Wraps.
    #[must_use]
    pub const fn received_total(&self) -> u64 {
        self.received_total
    }

    /// Capture at a frozen source high-water mark and host steady time.
    ///
    /// The caller holds the history lock, so later appends cannot enter the image.
    pub fn capture(
        &mut self,
        history: &CycleHistory,
        options: BindingOptions,
        capture_steady_nanos: u64,
    ) {
        self.capture_on(
            history,
            options,
            CaptureClock::Host { steady_nanos: capture_steady_nanos },
        );
    }

    /// Capture on `clock`. [`CaptureClock::Host`] is [`BindingCapture::capture`].
    pub fn capture_on(
        &mut self,
        history: &CycleHistory,
        options: BindingOptions,
        clock: CaptureClock,
    ) {
        let high_water = self.begin_capture(history, options, clock);
        match options.mode {
            CaptureMode::Latest => {
                self.capture_latest(history, high_water, clock);
            }
            CaptureMode::Queued => {
                let take = Take {
                    assigned: options.max_samples_per_iteration,
                    sent_through: None,
                    examine: Examine::WithinLimit,
                };
                self.capture_queued(history, high_water, options, clock, take);
            }
        }
    }

    /// Reset the per-capture results and judge freshness on `clock`.
    fn begin_capture(
        &mut self,
        history: &CycleHistory,
        options: BindingOptions,
        clock: CaptureClock,
    ) -> u64 {
        let high_water = history.high_water_next();
        self.received_total = accepted_on(history, clock);
        self.lost_since_capture = 0;
        self.stale_since_capture = 0;
        self.backlog_samples = 0;
        self.work_limit_drops = 0;
        self.samples.clear();
        self.judge(Some(history), options.silence_nanos, clock);
        high_water
    }

    /// The framework send stamps of the retained samples after this binding's cursor,
    /// oldest first.
    pub(crate) fn pending_stamps<'a>(
        &self,
        history: &'a CycleHistory,
    ) -> impl Iterator<Item = u64> + 'a + use<'a> {
        let start = history.cursor_offset(self.cursor);
        history.samples().range(start..).map(|sample| sample.framework_sent_nanos)
    }

    /// Host monotonic send time of the oldest retained sample after this binding's cursor.
    pub(crate) fn next_transport_sent_nanos(&self, history: &CycleHistory) -> Option<u64> {
        let start = history.cursor_offset(self.cursor);
        history.samples().get(start).map(|sample| sample.transport_sent_nanos)
    }

    /// Pending retained samples for this queued binding that `clock` admits.
    #[must_use]
    pub fn pending_samples_on(&self, history: &CycleHistory, clock: CaptureClock) -> usize {
        self.pending_arrivals(history, clock, None)
    }

    /// Pending retained samples this queued binding may take: those `clock` admits, and
    /// with `sent_through` only those within [`Self::reach`].
    pub(crate) fn pending_arrivals(
        &self,
        history: &CycleHistory,
        clock: CaptureClock,
        sent_through: Option<u64>,
    ) -> usize {
        self.pending_before(history, clock, self.reach(history, clock, sent_through))
    }

    /// The exclusive end of what a queued capture may take. Without `sent_through` it is
    /// the prefix `clock` admits. With it, it is the run of pending samples, in arrival
    /// order, up to the first one sent after that host monotonic time: a cursor takes
    /// samples in order, so a later sample ends the run even when an earlier-sent one
    /// follows it.
    fn reach(
        &self,
        history: &CycleHistory,
        clock: CaptureClock,
        sent_through: Option<u64>,
    ) -> Option<usize> {
        let Some(sent_through) = sent_through else {
            return clock.admitted_end(history);
        };
        let start = history.cursor_offset(self.cursor);
        let run = history
            .samples()
            .range(start..)
            .take_while(|sample| sample.transport_sent_nanos <= sent_through)
            .count();
        Some(start.saturating_add(run))
    }

    fn pending_before(
        &self,
        history: &CycleHistory,
        clock: CaptureClock,
        admitted_end: Option<usize>,
    ) -> usize {
        let start = history.cursor_offset(self.cursor);
        admitted_end.map_or_else(
            || history.samples().range(start..).filter(|sample| clock.admits(sample)).count(),
            |end| end.saturating_sub(start),
        )
    }

    /// Move the cursor up to `oldest` past evicted samples, and count them as lost.
    fn skip_evicted(&mut self, oldest: u64) {
        if before(self.cursor, oldest) {
            let lost = oldest.wrapping_sub(self.cursor);
            self.lost_since_capture = lost;
            self.counters.overrun_total = self.counters.overrun_total.wrapping_add(lost);
            self.cursor = oldest;
        }
    }

    /// Capture queued work assigned by the scheduler, on `clock`.
    pub fn capture_assigned_on(
        &mut self,
        history: &CycleHistory,
        options: BindingOptions,
        assigned: usize,
        clock: CaptureClock,
    ) {
        self.capture_arrivals(history, options, assigned, clock, None);
    }

    /// [`Self::capture_assigned_on`], taking with `sent_through` only the samples within
    /// [`Self::reach`]. The verdict and the received count are those of `clock`: a sample
    /// that waits behind `sent_through` has arrived, so its input is not silent.
    pub(crate) fn capture_arrivals(
        &mut self,
        history: &CycleHistory,
        options: BindingOptions,
        assigned: usize,
        clock: CaptureClock,
        sent_through: Option<u64>,
    ) {
        let high_water = self.begin_capture(history, options, clock);
        let take = Take { assigned, sent_through, examine: Examine::WithinLimit };
        self.capture_queued(history, high_water, options, clock, take);
    }

    /// Capture for a binding with no source at `clock`: what an empty history gives.
    pub fn capture_absent(&mut self, options: BindingOptions, clock: CaptureClock) {
        self.judge(None, options.silence_nanos, clock);
        if options.mode == CaptureMode::Latest {
            self.previous_latest_verdict = Some(self.verdict);
            self.latest_unavailable();
        }
    }

    fn capture_latest(&mut self, history: &CycleHistory, high_water: u64, clock: CaptureClock) {
        if clock.is_replay()
            && let Some(oldest) = history.samples().front().map(|sample| sample.sequence)
        {
            self.skip_evicted(oldest);
        }
        let end = clock.admitted_end(history).unwrap_or(history.samples().len());
        let selected = history.samples().range(..end).rev().find(|sample| {
            before(sample.sequence, high_water)
                && clock.admits(sample)
                && !history.is_rejected(sample.sequence)
        });
        // The verdict judged the newest admitted sample, or the held value when none.
        let usable = self.verdict == InputVerdict::Usable;
        match selected {
            None => {
                if !usable {
                    self.latest_unavailable();
                }
            }
            Some(sample) => {
                // The cursor never moves back: a rejected newer sample was passed already.
                let next = sample.sequence.wrapping_add(1);
                let replay_advance = clock.is_replay() && before(self.cursor, next);
                if replay_advance {
                    self.cursor = next;
                }
                if usable {
                    self.samples.push(Arc::clone(sample));
                    self.hold_latest_usable(sample);
                } else {
                    self.stale_since_capture = 1;
                    self.counters.age_rejections_total =
                        self.counters.age_rejections_total.wrapping_add(1);
                    self.latest_unavailable();
                }
            }
        }
        let latest_value_became_stale = self.verdict == InputVerdict::Stale
            && self.previous_latest_verdict == Some(InputVerdict::Usable);
        if latest_value_became_stale {
            self.counters.usable_to_stale_transitions =
                self.counters.usable_to_stale_transitions.wrapping_add(1);
        }
        self.previous_latest_verdict = Some(self.verdict);
    }

    /// Capture a trigger port of a triggered cycle on `clock`, judged as any input. When
    /// it `fires` it takes its oldest pending sample, delivered without a second age check:
    /// the schedule judged it. Its other samples stay pending.
    pub(crate) fn capture_trigger(
        &mut self,
        history: &CycleHistory,
        options: BindingOptions,
        fires: bool,
        clock: CaptureClock,
    ) {
        let high_water = self.begin_capture(history, options, clock);
        let options = BindingOptions {
            max_samples_per_iteration: 1,
            overflow_policy: QueueOverflowPolicy::Carry,
            ..options
        };
        let take =
            Take { assigned: usize::from(fires), sent_through: None, examine: Examine::Every };
        self.capture_queued(history, high_water, options, clock, take);
    }

    fn capture_queued(
        &mut self,
        history: &CycleHistory,
        high_water: u64,
        options: BindingOptions,
        clock: CaptureClock,
        take: Take,
    ) {
        let Take { assigned, sent_through, examine } = take;
        let Some(oldest) = history.samples().front().map(|sample| sample.sequence) else {
            return;
        };
        self.skip_evicted(oldest);

        let admitted_end = self.reach(history, clock, sent_through);
        let start = history.cursor_offset(self.cursor);
        let pending = self.pending_before(history, clock, admitted_end);
        let excess = pending.saturating_sub(assigned.min(options.max_samples_per_iteration));
        let dropped =
            if options.overflow_policy == QueueOverflowPolicy::Carry { 0 } else { excess };
        // Drop oldest: the cursor skips the excess.
        if dropped > 0 {
            let drop_through = match admitted_end {
                Some(_) => history.samples().get(start.saturating_add(dropped).saturating_sub(1)),
                None => history
                    .samples()
                    .range(start..)
                    .filter(|sample| clock.admits(sample))
                    .nth(dropped.saturating_sub(1)),
            };
            let Some(drop_through) = drop_through else {
                return;
            };
            self.cursor = drop_through.sequence.wrapping_add(1);
        }

        let mut examined = 0usize;
        let from = history.cursor_offset(self.cursor);
        let to = admitted_end.unwrap_or(history.samples().len()).max(from);
        for sample in history.samples().range(from..to) {
            let sample_is_outside_queued_window = !before_or_equal(self.cursor, sample.sequence)
                || !before(sample.sequence, high_water);
            if sample_is_outside_queued_window {
                continue;
            }
            // A replay cursor stops at its cut.
            if !clock.admits(sample) {
                break;
            }
            if examined >= assigned.min(options.max_samples_per_iteration) {
                break;
            }
            examined = examined.saturating_add(1);
            self.cursor = sample.sequence.wrapping_add(1);
            // A sample a reader rejected is neither delivered nor an age rejection.
            if history.is_rejected(sample.sequence) {
                continue;
            }
            let usable = match examine {
                Examine::WithinLimit => clock.usable(options.silence_nanos, sample),
                Examine::Every => true,
            };
            if usable {
                self.samples.push(Arc::clone(sample));
            } else {
                self.stale_since_capture = self.stale_since_capture.wrapping_add(1);
                self.counters.age_rejections_total =
                    self.counters.age_rejections_total.wrapping_add(1);
            }
        }

        self.work_limit_drops = u64::try_from(dropped).unwrap_or(u64::MAX);
        self.counters.work_limit_drops_total =
            self.counters.work_limit_drops_total.wrapping_add(self.work_limit_drops);

        if let Some(newest) = self.samples.last() {
            let usable_count = u64::try_from(self.samples.len()).unwrap_or(u64::MAX);
            self.delivered_total = self.delivered_total.wrapping_add(usable_count);
            self.held = Some(Arc::clone(newest));
        }
        self.backlog_samples = self.pending_arrivals(history, clock, sent_through);
    }

    /// A latest binding re-selects the same sample until a newer one arrives; only a new
    /// one counts as delivered.
    fn hold_latest_usable(&mut self, sample: &Arc<HistorySample>) {
        let already_held = self.held.as_ref().is_some_and(|held| held.sequence == sample.sequence);
        if already_held {
            return;
        }
        self.previous_held = self.held.replace(Arc::clone(sample));
        self.delivered_total = self.delivered_total.wrapping_add(1);
    }

    fn latest_unavailable(&mut self) {
        self.counters.latest_unavailable_iterations =
            self.counters.latest_unavailable_iterations.wrapping_add(1);
    }
}

/// What one queued capture takes.
#[derive(Debug, Clone, Copy)]
struct Take {
    /// The samples the scheduler assigned.
    assigned: usize,
    /// The host send time that bounds the capture ([`BindingCapture::reach`]), if any.
    sent_through: Option<u64>,
    /// Which of the examined samples are delivered.
    examine: Examine,
}

/// Which examined samples a queued capture delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Examine {
    /// Those within the silence limit; an older one is an age rejection.
    WithinLimit,
    /// Every one: the sample of a trigger step, which the schedule judged.
    Every,
}

/// The facts of one binding's held report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldReport {
    /// Samples the source had accepted at the capture. Wraps.
    pub received_total: u64,
    /// Usable samples that became the held sample. Wraps.
    pub delivered_total: u64,
    /// Sequence of the held sample, when one exists.
    pub held_sequence: Option<u64>,
}

/// The held report of `capture`.
#[must_use]
pub fn held_report(capture: &BindingCapture) -> HeldReport {
    HeldReport {
        received_total: capture.received_total(),
        delivered_total: capture.delivered_total(),
        held_sequence: capture.held().map(|sample| sample.sequence),
    }
}

/// Judge the newest value of one input at `at_nanos` on the host monotonic
/// clock. A zero silence limit keeps every earlier value usable.
#[must_use]
pub fn judge_value(silence_nanos: u64, sent_nanos: Option<u64>, at_nanos: u64) -> InputVerdict {
    let Some(sent) = sent_nanos else {
        return InputVerdict::NeverArrived;
    };
    if value_usable_within(silence_nanos, sent, at_nanos) {
        InputVerdict::Usable
    } else {
        InputVerdict::Stale
    }
}

fn before(sequence: u64, high_water: u64) -> bool {
    let sequence_precedes_high_water = sequence != high_water;
    let sequence_is_within_forward_half_range = high_water.wrapping_sub(sequence) < (1_u64 << 63);
    sequence_precedes_high_water && sequence_is_within_forward_half_range
}

fn before_or_equal(left: u64, right: u64) -> bool {
    let sequences_are_equal = left == right;
    sequences_are_equal || before(left, right)
}
