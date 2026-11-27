//! Managed input sources: bounded history fed by one `WaitSet` receiver.

use std::sync::{Arc, Mutex, Weak};

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
        subscriber.check_no_publisher_warning();
        // Zero: the receive reads the clock only once a frame arrives, so an idle input
        // reads no clock.
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
        if keep_newest {
            return self.drain_newest(subscriber, history, limit, &mut now);
        }
        drain_at_most(limit, || {
            let received = subscriber.receive_validated_with(&mut now, |frame| {
                let mut history = crate::locked(history);
                history.push_from(
                    frame.payload(),
                    frame.transport_sent_nanos(),
                    frame.framework_sent_nanos(),
                    origin_of(&frame),
                );
            });
            match received {
                Ok(Some(_)) | Err(crate::subscriber::ReceiveFailure::Refused(_)) => {
                    Ok(DrainStep::Taken)
                }
                Ok(None) => Ok(DrainStep::Empty),
                Err(crate::subscriber::ReceiveFailure::Transport(status)) => Err(status),
            }
        })
    }

    /// A host drain for `Latest` bindings only: only the newest frame is pushed. The
    /// frames it replaced count as accepted.
    fn drain_newest(
        &self,
        subscriber: &HyphaSubscriber,
        history: &Mutex<hypha_cycle::CycleHistory>,
        limit: usize,
        now: &mut u64,
    ) -> Result<(), Error> {
        let mut newest = crate::locked(&self.newest);
        newest.frames = 0;
        drain_at_most(limit, || {
            let received = subscriber.receive_validated_with(now, |frame| {
                newest.payload.clear();
                newest.payload.extend_from_slice(frame.payload());
                newest.transport_sent_nanos = frame.transport_sent_nanos();
                newest.framework_sent_nanos = frame.framework_sent_nanos();
                newest.origin = origin_of(&frame);
                newest.frames = newest.frames.saturating_add(1);
            });
            match received {
                Ok(Some(_)) | Err(crate::subscriber::ReceiveFailure::Refused(_)) => {
                    Ok(DrainStep::Taken)
                }
                Ok(None) => Ok(DrainStep::Empty),
                Err(crate::subscriber::ReceiveFailure::Transport(status)) => Err(status),
            }
        })?;
        if newest.frames == 0 {
            return Ok(());
        }
        let mut history = crate::locked(history);
        history.count_unstored(newest.frames.saturating_sub(1));
        history.push_from(
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
    let topic = endpoint.map_or_default(|endpoint| endpoint.topic.clone());
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
