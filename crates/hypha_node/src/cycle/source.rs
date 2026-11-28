//! Managed input sources: bounded history fed by one `WaitSet` receiver.

use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use super::registration::Cycle;
use crate::status::Error;
use crate::subscriber::HyphaSubscriber;
use crate::waitset::{HyphaWaitSet, WaitSetShared};
use hypha_cycle::ReplayFidelity;

/// The clock a drain serves, which decides how much it takes and what it keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrainClock {
    /// A step on the host clock.
    Host,
    /// A step or a turn under simulated time.
    Simulated,
}

/// What one receive of a bounded drain did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrainStep {
    /// A frame was taken from the queue, accepted or refused.
    Taken,
    /// The queue was empty.
    Empty,
}

/// Call `receive` until it finds the queue empty, at most `limit` times. A frame left
/// when the limit is reached stays queued for the next call.
pub(crate) fn drain_at_most<F>(limit: usize, mut receive: F) -> Result<(), Error>
where
    F: FnMut() -> Result<DrainStep, Error>,
{
    for _ in 0..limit {
        let step = receive()?;
        if step == DrainStep::Empty {
            return Ok(());
        }
    }
    Ok(())
}

/// An owned managed input source. Opaque to callers.
pub struct HyphaInputSource {
    pub(crate) inner: Arc<CycleInputSource>,
}

/// State shared by a source handle, its `WaitSet` receiver and bound cycles.
pub(crate) struct CycleInputSource {
    /// The shared state of the `WaitSet` that created this source; it does not keep the
    /// `WaitSet` alive.
    pub(crate) waitset: Weak<WaitSetShared>,
    /// The address of the `WaitSet`'s node, compared and never dereferenced.
    pub(crate) node: usize,
    /// The receiver this source drains, owned here. `None` for an unwired input.
    pub(crate) subscriber: Option<Arc<HyphaSubscriber>>,
    pub(crate) history: Option<Arc<Mutex<hypha_cycle::CycleHistory>>>,
    pub(crate) topic: String,
    /// The replay losses of this input, reported under simulated time.
    pub(crate) fidelity: Mutex<ReplayFidelity>,
    /// Queued (`Arrivals`) bindings ever made on this source. While there is none, a
    /// host-time drain stores only the newest sample. Never decremented.
    pub(crate) queued_bindings: std::sync::atomic::AtomicUsize,
    /// Whether a cycle committed on simulated time reads this source: its replay cuts
    /// admit older samples, so every drain keeps every sample, whoever drains it. Never
    /// cleared.
    pub(crate) replayed: std::sync::atomic::AtomicBool,
    /// The payload of the newest frame of a keep-newest drain, reused across drains.
    newest: Mutex<NewestFrame>,
    /// Samples a port of this source could not decode, each counted once.
    pub(crate) undecodable: std::sync::atomic::AtomicU64,
    /// When the undecodable line was last written: at most once per 10 s.
    pub(crate) undecodable_logged: Mutex<Option<std::time::Instant>>,
    /// The receives of one simulated-time drain: every publisher queue of the input, plus
    /// one. Fixed at creation: a wildcard resolves its topics once.
    sim_drain_limit: usize,
    /// `Latest` bindings ever made on this source. Never decremented.
    pub(crate) latest_bindings: std::sync::atomic::AtomicUsize,
    /// The samples of this source whose bytes the history reads in place, oldest first.
    pins: Mutex<hypha_cycle::PinLedger>,
    /// Samples read in place whose bytes a drain copied out, to free a slot for the next.
    pub(crate) pins_copied: std::sync::atomic::AtomicU64,
    /// When the copied line was last written, in host steady nanoseconds: at most once per 10 s.
    pins_copied_logged: Mutex<Option<i64>>,
}

/// The smallest message a host-clock drain reads in place rather than copies: below it a
/// copy costs less than a slot of the subscriber's borrow budget.
pub(crate) const PIN_MIN_BYTES: usize = 64 * 1024;

/// The samples one subscriber may hold borrowed at once; a drain keeps one free to receive.
const BORROWED_SAMPLES: usize = hypha_iceoryx2::MAX_BORROWED_SAMPLES;

/// A received sample whose bytes the history reads where they arrived.
struct InPlace(crate::subscriber::BackendSample);

impl hypha_cycle::PinnedBytes for InPlace {
    fn bytes(&self) -> &[u8] {
        self.0.payload()
    }
}

/// What a drain keeps of a frame besides its bytes.
#[derive(Clone, Copy)]
struct FrameStamps {
    transport_sent_nanos: u64,
    framework_sent_nanos: u64,
    origin: hypha_cycle::SampleOrigin,
}

impl FrameStamps {
    fn of(frame: &hypha_wire::frame::ParsedFrame<'_>) -> Self {
        Self {
            transport_sent_nanos: frame.transport_sent_nanos(),
            framework_sent_nanos: frame.framework_sent_nanos(),
            origin: origin_of(frame),
        }
    }
}

/// The newest frame a keep-newest drain has seen, and how many it replaced.
#[derive(Default)]
struct NewestFrame {
    payload: Vec<u8>,
    transport_sent_nanos: u64,
    framework_sent_nanos: u64,
    origin: hypha_cycle::SampleOrigin,
    frames: u64,
}

fn origin_of(frame: &hypha_wire::frame::ParsedFrame<'_>) -> hypha_cycle::SampleOrigin {
    hypha_cycle::SampleOrigin { publisher_id: frame.publisher_id(), sequence: frame.sequence() }
}

impl CycleInputSource {
    /// Whether this source was created on `set`. Compares addresses only; a destroyed
    /// `WaitSet`'s shared state stays allocated while this source names it.
    pub(crate) fn created_on(&self, set: &HyphaWaitSet) -> bool {
        std::ptr::eq(self.waitset.as_ptr(), Arc::as_ptr(&set.shared))
    }

    /// Keep room in the history for full subscriber queues plus `arrivals` samples
    /// that one step takes, so a queued binding never loses a sample to the history.
    pub(crate) fn reserve_arrivals(&self, arrivals: usize) {
        let queue = self.subscriber.as_deref().map_or(0, |s| s.port.topics_buffer_size());
        if let Some(history) = self.history.as_ref() {
            crate::locked(history).ensure_capacity(queue.saturating_add(arrivals));
        }
    }

    /// Drain this physical subscriber into its shared history, for `clock`.
    pub(crate) fn drain(&self, clock: DrainClock) -> Result<(), Error> {
        let Some(subscriber) = self.subscriber.as_deref() else {
            return if self.history.is_none() { Ok(()) } else { Err(Error::InvalidState) };
        };
        // Zero: the receive reads the clock only once a frame arrives, so an idle input
        // reads no clock. Its no-publisher check runs in the housekeeping of the turn.
        let mut now = 0;

        let Some(history) = self.history.as_ref() else {
            return Err(Error::InvalidState);
        };
        let sim = clock == DrainClock::Simulated;
        // One call takes everything queued when it starts: on host time one history depth;
        // under simulated time the queue of every publisher, so the progress reported
        // after a drain covers every sample sent before it (replay-flow-control.md §4.2).
        let limit = if sim {
            self.sim_drain_limit
        } else {
            subscriber.port.buffer_size().saturating_add(1)
        };
        let only_latest_bindings =
            self.queued_bindings.load(std::sync::atomic::Ordering::Acquire) == 0;
        // A replay cut admits older samples: a source that a replay cycle reads keeps
        // every one, also when a host-clock step drains it.
        let replayed = self.replayed.load(std::sync::atomic::Ordering::Acquire);
        let keep_newest = only_latest_bindings && !sim && !replayed;
        // On the host clock, outside a replay, a large message is read in place.
        let in_place = !sim && !replayed;
        if keep_newest {
            return self.drain_newest(subscriber, history, limit, &mut now);
        }
        drain_at_most(limit, || {
            if in_place {
                self.free_a_slot(history, false);
            }
            let received = subscriber.receive_validated_with(&mut now, |frame| {
                let stamps = FrameStamps::of(&frame);
                let pin = in_place && frame.payload().len() >= PIN_MIN_BYTES;
                if !pin {
                    crate::locked(history).push_from(
                        frame.payload(),
                        stamps.transport_sent_nanos,
                        stamps.framework_sent_nanos,
                        stamps.origin,
                    );
                }
                (stamps, pin)
            });
            match received {
                Ok(Some((sample, (stamps, true)))) => {
                    self.push_in_place(history, sample, stamps);
                    Ok(DrainStep::Taken)
                }
                Ok(Some(_)) | Err(crate::subscriber::ReceiveFailure::Refused(_)) => {
                    Ok(DrainStep::Taken)
                }
                Ok(None) => Ok(DrainStep::Empty),
                Err(crate::subscriber::ReceiveFailure::Transport(status)) => Err(status),
            }
        })
    }

    /// Push `sample` into `history` with its bytes in place.
    fn push_in_place(
        &self,
        history: &Mutex<hypha_cycle::CycleHistory>,
        sample: crate::subscriber::BackendSample,
        stamps: FrameStamps,
    ) {
        let payload = hypha_cycle::PinnedPayload::new(Box::new(InPlace(sample)));
        crate::locked(&self.pins).track(&payload);
        crate::locked(history).push_pinned(
            payload,
            stamps.transport_sent_nanos,
            stamps.framework_sent_nanos,
            stamps.origin,
        );
    }

    /// Before a receive: give back the slots of samples no binding reads again, then copy out
    /// the oldest until a slot is free for the receive and for `holding`, a newer sample the
    /// drain holds in hand. A sample is read again while a queued binding has not taken it,
    /// or while it is the newest a `Latest` binding holds and nothing newer is in hand.
    fn free_a_slot(&self, history: &Mutex<hypha_cycle::CycleHistory>, holding: bool) {
        let mut pins = crate::locked(&self.pins);
        if pins.is_empty() {
            return;
        }
        let queued = self.queued_bindings.load(std::sync::atomic::Ordering::Acquire);
        let readers = hypha_cycle::PinReaders {
            queued: u32::try_from(queued).unwrap_or(u32::MAX),
            latest: self.latest_bindings.load(std::sync::atomic::Ordering::Acquire) > 0,
            holding,
            slots: BORROWED_SAMPLES,
        };
        let newest = crate::locked(history)
            .samples()
            .back()
            .and_then(|sample| sample.payload.as_pinned().cloned());
        let copied = pins.free_a_slot(newest.as_ref(), readers);
        drop(pins);
        for _ in 0..copied {
            self.note_copied();
        }
    }

    /// Count a sample read in place that a drain copied out, and say so at most every 10 s.
    fn note_copied(&self) {
        let total = self.pins_copied.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut logged = crate::locked(&self.pins_copied_logged);
        let now = hypha_time::steady_now().nanos();
        let due = logged.is_none_or(|at| now.saturating_sub(at) >= 10_000_000_000);
        if due {
            *logged = Some(now);
            log::warn!(target: "hypha/cycle", "hypha: input '{}': a sample read in place was copied, to free a slot of its subscriber for the next (copied {} so far); the step takes fewer samples than arrive", self.topic, total.saturating_add(1));
        }
    }

    /// A host drain for `Latest` bindings only: only the newest frame is pushed. The
    /// frames it replaced count as accepted. A large newest frame is kept in hand and pushed
    /// in place; a smaller one is copied once into a reused buffer.
    fn drain_newest(
        &self,
        subscriber: &HyphaSubscriber,
        history: &Mutex<hypha_cycle::CycleHistory>,
        limit: usize,
        now: &mut u64,
    ) -> Result<(), Error> {
        let mut newest = crate::locked(&self.newest);
        newest.frames = 0;
        let mut in_hand: Option<(crate::subscriber::BackendSample, FrameStamps)> = None;
        drain_at_most(limit, || {
            self.free_a_slot(history, in_hand.is_some());
            let received = subscriber.receive_validated_with(now, |frame| {
                let stamps = FrameStamps::of(&frame);
                let pin = frame.payload().len() >= PIN_MIN_BYTES;
                if !pin {
                    newest.payload.clear();
                    newest.payload.extend_from_slice(frame.payload());
                    newest.transport_sent_nanos = stamps.transport_sent_nanos;
                    newest.framework_sent_nanos = stamps.framework_sent_nanos;
                    newest.origin = stamps.origin;
                }
                newest.frames = newest.frames.saturating_add(1);
                (stamps, pin)
            });
            match received {
                Ok(Some((sample, (stamps, pin)))) => {
                    // The frame before it is replaced: its slot goes back.
                    in_hand = pin.then_some((sample, stamps));
                    Ok(DrainStep::Taken)
                }
                Err(crate::subscriber::ReceiveFailure::Refused(_)) => Ok(DrainStep::Taken),
                Ok(None) => Ok(DrainStep::Empty),
                Err(crate::subscriber::ReceiveFailure::Transport(status)) => Err(status),
            }
        })?;
        if newest.frames == 0 {
            return Ok(());
        }
        crate::locked(history).count_unstored(newest.frames.saturating_sub(1));
        if let Some((sample, stamps)) = in_hand {
            drop(newest);
            self.push_in_place(history, sample, stamps);
            return Ok(());
        }
        crate::locked(history).push_from(
            &newest.payload,
            newest.transport_sent_nanos,
            newest.framework_sent_nanos,
            newest.origin,
        );
        Ok(())
    }

    /// Cumulative missing forward sequence values accepted by this source's
    /// validator, or zero for a disabled source.
    pub(crate) fn missing_sequences(&self) -> u64 {
        self.subscriber.as_deref().map_or(0, HyphaSubscriber::missing_sequences)
    }
}

/// The samples of `source` whose bytes its history reads in place now, and those a drain
/// copied out to free a slot. Not part of the C ABI: for the tests of `hypha_ffi`.
#[doc(hidden)]
#[must_use]
pub fn input_source_in_place(source: &HyphaInputSource) -> (usize, u64) {
    let in_place = crate::locked(&source.inner.pins).in_place();
    (in_place, source.inner.pins_copied.load(std::sync::atomic::Ordering::Acquire))
}

/// Creates an input source of `set` that drains `subscriber` alone, or an unwired
/// input for `None`. The source owns the subscriber from then on; a refusal gives it
/// back with the error.
///
/// # Errors
/// `InvalidArgument` for a subscriber of another node or a queue too deep for a
/// history; `InvalidOperation` for a subscriber that is already a source;
/// `ResourceLimit` when the source list cannot grow; `InvalidState` while `set` runs;
/// an error of the subscriber's claim.
#[allow(clippy::type_complexity, reason = "the refused subscriber goes back with the error")]
pub fn input_source_create(
    set: &HyphaWaitSet,
    subscriber: Option<Arc<HyphaSubscriber>>,
) -> Result<Box<HyphaInputSource>, (Error, Option<Arc<HyphaSubscriber>>)> {
    match check_input_source(set, subscriber.as_deref()) {
        Ok(checked) => Ok(Box::new(add_input_source(set, subscriber, checked))),
        Err(error) => Err((error, subscriber)),
    }
}

struct CheckedSource<'set> {
    history: Option<Arc<Mutex<hypha_cycle::CycleHistory>>>,
    topic: String,
    input_sources: std::cell::RefMut<'set, Vec<std::sync::Weak<CycleInputSource>>>,
}

fn check_input_source<'set>(
    set: &'set HyphaWaitSet,
    endpoint: Option<&HyphaSubscriber>,
) -> Result<CheckedSource<'set>, Error> {
    let history = match endpoint {
        None => None,
        Some(endpoint) => {
            // The queue plus one slot, so a drain of a full queue keeps the newest sample of
            // the drain before it; a wildcard has one queue per topic. In replay every turn
            // drains the queues, which then hold one chunk of each topic, and a `Latest`
            // history keeps it and the newest sample before it
            // (replay-flow-control.md §6.7). A queued binding adds its `perStep`
            // (`reserve_arrivals`).
            let depth = endpoint.port.topics_buffer_size().saturating_add(1);
            let history = hypha_cycle::CycleHistory::new(depth).ok_or(Error::InvalidArgument)?;
            Some(Arc::new(Mutex::new(history)))
        }
    };
    let mut input_sources = set.input_sources_mut()?;
    input_sources.retain(|source| source.strong_count() > 0);
    if input_sources.try_reserve(1).is_err() {
        return Err(Error::ResourceLimit);
    }
    let node = set.node_address();
    if endpoint.is_some_and(|endpoint| endpoint.owner_node != node) {
        return Err(Error::InvalidArgument);
    }
    let topic = endpoint.map(|endpoint| endpoint.topic.clone()).unwrap_or_default();
    // The source drains the subscriber alone.
    if let Some(endpoint) = endpoint {
        let previous = endpoint.claim(crate::subscriber::MODE_SOURCE)?;
        let already_a_source = previous == crate::subscriber::MODE_SOURCE;
        if already_a_source {
            return Err(Error::InvalidOperation);
        }
    }
    Ok(CheckedSource { history, topic, input_sources })
}

/// The source owns the subscriber from here. Cycle ticks drain it before capture, so
/// it needs no `WaitSet` registration.
fn add_input_source(
    set: &HyphaWaitSet,
    subscriber: Option<Arc<HyphaSubscriber>>,
    checked: CheckedSource<'_>,
) -> HyphaInputSource {
    let CheckedSource { history, topic, mut input_sources } = checked;
    let sim_drain_limit =
        subscriber.as_deref().map_or(0, |s| s.port.queue_capacity().saturating_add(1));
    let inner = Arc::new(CycleInputSource {
        waitset: Arc::downgrade(&set.shared),
        node: set.node_address(),
        subscriber,
        history,
        topic,
        fidelity: Mutex::new(ReplayFidelity::default()),
        queued_bindings: std::sync::atomic::AtomicUsize::new(0),
        replayed: std::sync::atomic::AtomicBool::new(false),
        newest: Mutex::new(NewestFrame::default()),
        undecodable: std::sync::atomic::AtomicU64::new(0),
        undecodable_logged: Mutex::new(None),
        sim_drain_limit,
        latest_bindings: std::sync::atomic::AtomicUsize::new(0),
        pins: Mutex::new(hypha_cycle::PinLedger::default()),
        pins_copied: std::sync::atomic::AtomicU64::new(0),
        pins_copied_logged: Mutex::new(None),
    });
    input_sources.push(Arc::downgrade(&inner));
    HyphaInputSource { inner }
}

/// Drops the caller's handle of an input source; a cycle that binds it keeps it.
///
/// # Errors
/// `InvalidState` while its `WaitSet` runs and no cycle binds the source; the handle
/// comes back with the error.
pub fn input_source_destroy(
    source: Box<HyphaInputSource>,
) -> Result<(), (Error, Box<HyphaInputSource>)> {
    // A handle that is the last owner frees the receiver and the history, which a run
    // may be using. A bound cycle is another owner, and no cycle is destroyed during a
    // run, so the handle of a bound source only gives up its share.
    let last_owner = Arc::strong_count(&source.inner) == 1;
    if !last_owner {
        drop(source);
        return Ok(());
    }
    let Some(shared) = source.inner.waitset.upgrade() else {
        drop(source);
        return Ok(());
    };
    // Claimed, so no run starts while the receiver and the history are freed.
    let claimed = shared.claim();
    if claimed.is_err() || shared.running.is_running() {
        drop(claimed);
        return Err((Error::InvalidState, source));
    }
    drop(source);
    drop(claimed);
    Ok(())
}

/// Drain each physical source of `cycle` once into its history for `clock`, for the
/// dispatcher: before a step it cuts itself, and on every turn under simulated time.
/// `InvalidState` inside a step.
pub(crate) fn drain_inputs(cycle: &Cycle, clock: DrainClock) -> Result<(), Error> {
    drain_sources(&cycle.step_ref()?.inputs, clock)
}

/// Check the stream of each input of `cycle` that has a subscriber, at host time
/// `now_nanos`, from the housekeeping of a turn: never inside its step. A cycle whose step
/// runs is skipped. On simulated time (`replay`), warn too of an input with a second
/// publisher, at most once per second per input.
pub(crate) fn check_input_publishers(cycle: &Cycle, now_nanos: u64, replay: bool) {
    let Ok(step) = cycle.step_ref() else { return };
    for source in &step.inputs {
        let Some(subscriber) = source.subscriber.as_deref() else { continue };
        subscriber.check_no_publisher_warning(now_nanos);
        if replay {
            warn_of_several_publishers(source, subscriber);
        }
    }
}

/// Warn when the replay input `source` has more than one live publisher: the order of
/// their samples depends on turn timing (`replay-flow-control.md` section 6.7).
fn warn_of_several_publishers(source: &CycleInputSource, subscriber: &crate::HyphaSubscriber) {
    let mut fidelity = crate::locked(&source.fidelity);
    if !fidelity.publishers_due(Instant::now()) {
        return;
    }
    match crate::subscriber::subscriber_publisher_count(subscriber) {
        Ok(count) => {
            if let Some(text) = fidelity.record_publishers(&source.topic, count) {
                log::warn!(target: "hypha", "hypha: {text}");
            }
        }
        Err(error) => {
            log::warn!(target: "hypha", "hypha: cannot count the publishers of {}: {error:?}", source.topic);
        }
    }
}

/// Drain each physical source once into its history.
///
/// Several bindings and cycles can share a source, whose history is their common snapshot.
pub(super) fn drain_sources(
    inputs: &[Arc<CycleInputSource>],
    clock: DrainClock,
) -> Result<(), Error> {
    for (index, source) in inputs.iter().enumerate() {
        let already_drained = inputs
            .get(..index)
            .is_some_and(|previous| previous.iter().any(|item| Arc::ptr_eq(item, source)));
        if !already_drained {
            source.drain(clock)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DrainStep, drain_at_most};
    use crate::status::Error;

    #[test]
    fn a_drain_takes_at_most_its_history_depth_per_call() {
        let mut calls = 0_usize;
        let result = drain_at_most(5, || {
            calls += 1;
            Ok(DrainStep::Taken)
        });
        assert_eq!(result, Ok(()));
        assert_eq!(calls, 5, "a queue that never empties stops the call at the limit");
    }

    #[test]
    fn a_sample_left_by_a_bounded_drain_is_taken_by_the_next_call() {
        let queued = std::cell::Cell::new(7_usize);
        let take = || {
            let left = queued.get();
            if left == 0 {
                return Ok(DrainStep::Empty);
            }
            queued.set(left - 1);
            Ok(DrainStep::Taken)
        };
        assert_eq!(drain_at_most(5, take), Ok(()));
        assert_eq!(queued.get(), 2, "two samples stay queued");
        assert_eq!(drain_at_most(5, take), Ok(()));
        assert_eq!(queued.get(), 0, "the next call takes them");
    }

    #[test]
    fn a_failed_receive_ends_the_drain_with_its_status() {
        let result = drain_at_most(5, || Err(Error::Backend));
        assert_eq!(result, Err(Error::Backend));
    }
}
