//! Caller-owned execution: the application attaches work to a configuration and
//! calls [`Iox2WaitSet::run`] on a thread it owns. This module creates no thread.
//!
//! Only a listener is attachable, and a guard borrows its listener for the whole
//! run, so listeners live apart from the endpoints the dispatcher mutates.

mod deadline;
mod run;

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::sync::Arc;

use iceoryx2::port::listener::Listener;
use iceoryx2::port::notifier::Notifier;

use hypha_core::port::PortError;
use hypha_core::port::dispatch::{AttachmentId, RunReport, next_free_identifier};

use crate::transport::Ipc;

/// The listener of one subscriber's event service, as `Iox2WaitSet::attach` takes it.
pub type Iox2Listener = Listener<Ipc>;

/// When a registration runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Run when the listener is notified.
    Event,
    /// Run on a fixed period, whatever the publisher does.
    Periodic(Duration),
    /// Run when the dispatcher's own deadline is due, with no native guard (see
    /// [`Dispatcher::next_deadline`]).
    Timed,
}

/// What the dispatcher asks the run loop to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progression {
    /// Keep running.
    Continue,
    /// Stop this run and report a requested stop.
    Stop,
    /// Stop this run and report a failure.
    Fail(PortError),
}

/// What one event turn achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOutcome {
    /// The attachment has nothing left. It waits for the next event.
    Drained,
    /// Work remains. Run this attachment again without another event.
    MoreWork,
    /// Stop this run and report a requested stop.
    Stop,
    /// Stop this run and report a failure.
    Fail(PortError),
}

/// What the run loop reports to the application; borrowed for the whole run.
pub trait Dispatcher {
    /// An attachment is ready. Take one bounded turn of work from it.
    fn on_event(&mut self, attachment: AttachmentId) -> EventOutcome;

    /// A periodic registration is due. `missed` counts skipped periods.
    fn on_interval(&mut self, _attachment: AttachmentId, _missed: u64) -> Progression {
        Progression::Continue
    }

    /// Runs before every bounded dispatch turn, so work it publishes goes out the same turn.
    fn on_timers(&mut self) -> Progression {
        Progression::Continue
    }

    /// Runs after every bounded dispatch turn, whatever else happened.
    fn on_tick(&mut self) -> Progression {
        Progression::Continue
    }

    /// The earliest [`Trigger::Timed`] deadline on the host's monotonic clock
    /// ([`hypha_time::steady_now`]), or `None`. A deadline that has passed is due now.
    fn next_deadline(&mut self) -> Option<hypha_time::Time> {
        None
    }
}

/// How many registrations one `WaitSet` accepts, including internal ones.
pub const MAX_REGISTRATIONS: usize = 128;

/// The longest native wait. A signal delivered to another thread is seen within it.
const IDLE_BACKSTOP: Duration = Duration::from_secs(1);

/// One registration. The `WaitSet` holds the listener; the dispatcher the endpoint.
struct Registration {
    id: AttachmentId,
    trigger: Trigger,
}

/// The per-registration state one run carries between its turns, index-aligned
/// with the run's registrations.
struct TurnState {
    /// The registration has work waiting for its next turn.
    ready: Vec<bool>,
    /// Periodic ticks observed but not yet reported.
    due: Vec<u64>,
}

impl TurnState {
    /// Every attachment starts ready, so data queued at run entry is delivered.
    fn new(count: usize) -> Self {
        Self { ready: vec![true; count], due: vec![0; count] }
    }

    fn has_pending_work(&self) -> bool {
        self.ready.iter().any(|slot| *slot)
    }
}

struct Control {
    listener: Listener<Ipc>,
    notifier: Arc<Notifier<Ipc>>,
}

/// A caller-owned `WaitSet`. It owns its registrations and creates no thread.
pub struct Iox2WaitSet {
    /// Index-aligned with `registrations`. A guard borrows an entry here for
    /// the whole run, so nothing in this collection may be mutated during one.
    listeners: Vec<Vec<Listener<Ipc>>>,
    registrations: Vec<Registration>,
    control: Control,
    /// Ends a run's wait at the dispatcher's timed deadline.
    timer: deadline::DeadlineTimer,
    stop: Arc<AtomicBool>,
    /// The identifier handed out last.
    last_id: AttachmentId,
}

impl Iox2WaitSet {
    /// Create a `WaitSet` on `node`. The node lock is not held while the
    /// control service is opened.
    ///
    /// # Errors
    /// Returns [`PortError`] when the control event service cannot be opened, or
    /// [`PortError::Backend`] when the deadline timer cannot be created.
    pub(crate) fn new(
        node: &crate::transport::SharedNode,
        control_service: &str,
    ) -> Result<Self, PortError> {
        let limits = Some(crate::notification::EventLimits::CONTROL);
        let listener = crate::notification::create_listener(node, control_service, limits)?;
        let notifier = crate::notification::create_notifier(node, control_service, limits)?;
        Ok(Self {
            listeners: Vec::with_capacity(MAX_REGISTRATIONS),
            registrations: Vec::with_capacity(MAX_REGISTRATIONS),
            control: Control { listener, notifier: Arc::new(notifier) },
            timer: deadline::DeadlineTimer::new()?,
            stop: Arc::new(AtomicBool::new(false)),
            last_id: 0,
        })
    }

    /// A handle that stops this `WaitSet` from another thread.
    #[must_use]
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle { stop: Arc::clone(&self.stop), notifier: Arc::clone(&self.control.notifier) }
    }

    /// The next free id, at most `LAST_NATIVE_IDENTIFIER`: the ids above it name the
    /// registrations a dispatcher holds alone, which this set never sees.
    fn claim_id(&mut self) -> Result<AttachmentId, PortError> {
        let registrations = &self.registrations;
        let in_use =
            |candidate: AttachmentId| registrations.iter().any(|held| held.id == candidate);
        let id = next_free_identifier(self.last_id, registrations.len(), in_use)
            .ok_or(PortError::ResourceLimit)?;
        self.last_id = id;
        Ok(id)
    }

    /// Attach `listeners` under `trigger`: an event on any of them makes the registration
    /// ready. A trigger that needs a listener refuses none rather than poll.
    ///
    /// # Errors
    /// Returns [`PortError::ResourceLimit`] when the registration limit is
    /// reached, or [`PortError::InvalidTopic`] when the trigger needs a
    /// listener that was not supplied.
    pub fn attach(
        &mut self,
        listeners: Vec<Listener<Ipc>>,
        trigger: Trigger,
    ) -> Result<AttachmentId, PortError> {
        if self.registrations.len() >= MAX_REGISTRATIONS {
            return Err(PortError::ResourceLimit);
        }
        if trigger == Trigger::Event && listeners.is_empty() {
            return Err(PortError::InvalidTopic);
        }
        let id = self.claim_id()?;
        self.listeners.push(listeners);
        self.registrations.push(Registration { id, trigger });
        Ok(id)
    }

    /// Remove a registration. Only permitted while the `WaitSet` is stopped.
    ///
    /// # Errors
    /// Returns [`PortError::InvalidTopic`] when no registration has this id.
    pub fn detach(&mut self, attachment: AttachmentId) -> Result<(), PortError> {
        let index = self
            .registrations
            .iter()
            .position(|registration| registration.id == attachment)
            .ok_or(PortError::InvalidTopic)?;
        self.registrations.remove(index);
        self.listeners.remove(index);
        Ok(())
    }

    /// How many registrations this `WaitSet` holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.registrations.len()
    }

    /// True when this `WaitSet` holds no registration.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }

    /// Ask a running `WaitSet` to stop. Idempotent.
    ///
    /// # Errors
    /// Returns [`PortError::Disconnected`] when the wake-up cannot be sent. The
    /// stop flag is still set, so the next wait slice observes it.
    pub fn request_stop(&self) -> Result<(), PortError> {
        self.stop.store(true, Ordering::SeqCst);
        self.control.notifier.notify().map(|_| ()).map_err(|_| PortError::Disconnected)
    }

    /// Run on the calling thread until a stop or a failure.
    ///
    /// Every native attachment is created before the first callback; a setup failure
    /// runs no callback. `budget` bounds the turns before native readiness is checked again.
    #[must_use]
    pub fn run(&mut self, budget: u32, dispatcher: &mut impl Dispatcher) -> RunReport {
        self.run_bounded(budget, None, dispatcher)
    }

    /// Run for at most `limit`, then stop as if the application had asked.
    /// One blocking run with a deadline: the native `WaitSet` stays on one run stack.
    #[must_use]
    pub fn run_for(
        &mut self,
        budget: u32,
        limit: Duration,
        dispatcher: &mut impl Dispatcher,
    ) -> RunReport {
        let deadline = std::time::Instant::now().checked_add(limit);
        self.run_bounded(budget, deadline, dispatcher)
    }
}

/// Stops a running `WaitSet` from another thread.
#[derive(Clone)]
pub struct StopHandle {
    stop: Arc<AtomicBool>,
    notifier: Arc<Notifier<Ipc>>,
}

impl StopHandle {
    /// Ask the `WaitSet` to stop. Idempotent.
    ///
    /// # Errors
    /// Returns [`PortError::Disconnected`] when the wake-up cannot be sent. The
    /// stop flag is still set, so the next wait slice observes it.
    pub fn request_stop(&self) -> Result<(), PortError> {
        self.stop.store(true, Ordering::SeqCst);
        self.notifier.notify().map(|_| ()).map_err(|_| PortError::Disconnected)
    }
}
