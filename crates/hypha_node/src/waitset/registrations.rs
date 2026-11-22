//! `WaitSet` registrations: the one C ABI entry that attaches work, its
//! per-kind checks, and the shared admission check.
//!
//! `hypha_waitset_attach`: a field that `kind` does not use must hold its cleared value,
//! otherwise the attach is refused with `InvalidArgument`. A successful attach transfers
//! `context` to the registration, which calls `release` once when it is detached or the
//! `WaitSet` is destroyed; a failed attach transfers nothing.
//!
//! During a run the native set is the run's, so only a timer that needs no native
//! attachment is accepted, from a callback: the dispatcher alone holds it, under an id
//! above every native one
//! ([`FIRST_DISPATCHER_IDENTIFIER`](hypha_core::port::dispatch::FIRST_DISPATCHER_IDENTIFIER)).

use super::timer_control::{TimerControl, clock_now};
use super::{
    Arc, DataCallback, DeliveryHandler, Duration, Error, ForeignCallbacks, HyphaAttachmentId,
    HyphaContextRelease, HyphaIntervalCallback, HyphaSubscriber, HyphaSubscriptionCallback,
    HyphaWaitSet, IntervalHandler, Registration, SubscriptionHandler, Target, Trigger, c_void,
};
use crate::cycle::{Cycle, CycleId};
use hypha_core::port::dispatch::LAST_NATIVE_IDENTIFIER;

/// How often a wildcard subscription, which has no event service, is read.
const WILDCARD_POLL_PERIOD: Duration = Duration::from_millis(1);

/// A subscription that runs when its subscriber receives a sample.
pub const HYPHA_ATTACH_SUBSCRIPTION: u32 = 0;
/// A periodic registration on framework time, owned by the `WaitSet`. Its callback gets
/// the periods missed since its previous call (0 on time; not a running total). Simulated
/// time earlier than the timer has seen fires nothing and ends the run with `ClockRewound`,
/// canceled or not. A callback of the running `WaitSet` may attach one when
/// the node runs on host time or on its own simulated clock; its first call comes one
/// period after the attach.
pub const HYPHA_ATTACH_TIMER: u32 = 2;

/// Describes one registration. `kind` is one of the `HYPHA_ATTACH_*` values.
/// Each kind reads only the fields its documentation names; every other field must
/// hold its cleared value. A subscription shares its subscriber until the detach; the
/// caller's `hypha_subscriber_destroy` releases only its own handle.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HyphaAttachment {
    /// One of the `HYPHA_ATTACH_*` values.
    pub kind: u32,
    /// Caller context for the callback. Transferred on a successful attach.
    pub context: *mut c_void,
    /// Releases `context`. May be null when `context` needs no release.
    pub release: HyphaContextRelease,
    /// Subscription: the subscriber. The registration shares it until the detach.
    pub subscriber: *mut HyphaSubscriber,
    /// Subscription: called for each sample the subscription accepts.
    pub subscription_callback: HyphaSubscriptionCallback,
    /// Timer: the period in nanoseconds. Must be positive and at most the largest
    /// framework duration (`InvalidArgument` otherwise).
    pub period_nanos: u64,
    /// Timer: called once for each due period.
    pub periodic_callback: HyphaIntervalCallback,
}

/// The callback and context of a C registration, not yet owned: `adopt` turns them into
/// their owner once the attach can no longer fail.
pub(crate) struct Adoption<T: ?Sized> {
    attachment: HyphaAttachment,
    adopt: fn(&HyphaAttachment) -> Box<T>,
}

impl<T: ?Sized> Adoption<T> {
    fn adopt(self) -> Box<T> {
        (self.adopt)(&self.attachment)
    }
}

pub(crate) enum SubscriptionCallback {
    /// A Rust handler.
    Native(Box<dyn SubscriptionHandler>),
    /// A C callback with its context.
    Foreign(Adoption<dyn DeliveryHandler>),
}

/// A registration, decoded and checked.
pub(crate) enum Request {
    Subscription {
        subscriber: Arc<HyphaSubscriber>,
        callback: SubscriptionCallback,
    },
    /// A timer that calls a C callback, or, for a cycle timer, steps `cycle`.
    Timer {
        period: Duration,
        framework_period: hypha_time::Duration,
        callback: Option<Adoption<dyn IntervalHandler>>,
        cycle: Option<CycleId>,
    },
    /// The periodic wake of a triggered cycle, whose trigger falls silent after
    /// `silence_nanos`.
    Triggered {
        cycle: CycleId,
        silence_nanos: i64,
        /// Whether the trigger topic has a listener that wakes the `WaitSet`.
        notified: bool,
    },
    /// A cycle that drives `/clock`.
    Driver(Box<super::clock_driver::DriverCycle>),
    /// The listener on the trigger topic of a triggered cycle.
    TriggerWake {
        listener: Box<hypha_iceoryx2::Iox2Listener>,
    },
}

impl Request {
    /// Check the fields that `attachment.kind` selects, and refuse any field it
    /// does not use. `share` runs only once every field passed.
    pub(crate) fn decode<C: ForeignCallbacks>(
        attachment: &HyphaAttachment,
        share: impl FnOnce(*mut HyphaSubscriber) -> Arc<HyphaSubscriber>,
    ) -> Result<Self, Error> {
        match attachment.kind {
            HYPHA_ATTACH_SUBSCRIPTION => {
                let stray: bool = !periodic_fields_clear(attachment);
                let refused: bool = attachment.subscriber.is_null()
                    || attachment.subscription_callback.is_none()
                    || stray;
                if refused {
                    return Err(Error::InvalidArgument);
                }
                let callback = Adoption { attachment: *attachment, adopt: C::subscription };
                Ok(Self::Subscription {
                    subscriber: share(attachment.subscriber),
                    callback: SubscriptionCallback::Foreign(callback),
                })
            }
            HYPHA_ATTACH_TIMER => {
                let stray: bool = !subscription_fields_clear(attachment);
                let refused: bool = attachment.periodic_callback.is_none() || stray;
                if refused {
                    return Err(Error::InvalidArgument);
                }
                let callback = Adoption { attachment: *attachment, adopt: C::timer };
                Self::timer(attachment.period_nanos, Some(callback), None)
            }
            _ => Err(Error::InvalidArgument),
        }
    }

    fn timer(
        period_nanos: u64,
        callback: Option<Adoption<dyn IntervalHandler>>,
        cycle: Option<CycleId>,
    ) -> Result<Self, Error> {
        Ok(Self::Timer {
            period: Duration::from_nanos(period_nanos),
            framework_period: framework_period(period_nanos)?,
            callback,
            cycle,
        })
    }
}

/// A period as a framework-time duration, which holds an i64 count.
fn framework_period(period_nanos: u64) -> Result<hypha_time::Duration, Error> {
    let framework_nanos = i64::try_from(period_nanos)
        .ok()
        .filter(|nanos| *nanos > 0)
        .ok_or(Error::InvalidArgument)?;
    Ok(hypha_time::Duration::from_nanos(framework_nanos))
}

fn subscription_fields_clear(attachment: &HyphaAttachment) -> bool {
    attachment.subscriber.is_null() && attachment.subscription_callback.is_none()
}

fn periodic_fields_clear(attachment: &HyphaAttachment) -> bool {
    attachment.period_nanos == 0 && attachment.periodic_callback.is_none()
}

/// Attach a decoded request to `set` and record it.
///
/// The one attach path, so every registration passes the same admission check
/// before any native attach.
pub(crate) fn attach(set: &HyphaWaitSet, request: Request) -> Result<HyphaAttachmentId, Error> {
    if set.is_running() {
        return defer_attach(set, request);
    }
    // Held across the native attach, so an accepted registration is always recorded.
    let mut registrations = set.registrations.try_borrow_mut().map_err(|_| Error::InvalidState)?;
    if registrations.len() >= registration_limit(set) {
        return Err(Error::ResourceLimit);
    }
    // Borrowed before the native attach, so nothing fails after the registration exists.
    let mut live = set.live.try_borrow_mut().map_err(|_| Error::InvalidState)?;
    let (id, target) = register(set, request)?;
    let registration = Registration::new(id, target);
    live.push(registration.live_entry());
    registrations.push(registration);
    Ok(id)
}

fn registration_limit(set: &HyphaWaitSet) -> usize {
    usize::try_from(set.limits.max_registrations()).unwrap_or(usize::MAX)
}

/// An attach during a run: only a plain timer whose wake needs no native attachment, from
/// the run's own thread. The dispatcher takes it before its next wait; its first call
/// comes one period after now. `InvalidState` for anything else.
fn defer_attach(set: &HyphaWaitSet, request: Request) -> Result<HyphaAttachmentId, Error> {
    let from_the_run = set.shared.run_owner() == Some(std::thread::current().id());
    if !from_the_run {
        return Err(Error::InvalidState);
    }
    // A refused request drops here and leaves its C context to the caller.
    let Request::Timer { period, framework_period, callback: Some(callback), cycle: None } =
        request
    else {
        return Err(Error::InvalidState);
    };
    let node = set.node();
    let sim_time = node.on_sim_time();
    // A clock driven from outside the node needs a native poll, and the run holds the set.
    if timer_wake(period, node.uses_sim_time, sim_time) != Trigger::Timed {
        return Err(Error::InvalidState);
    }
    let (Ok(mut attaches), Ok(mut live)) =
        (set.run_attaches.try_borrow_mut(), set.live.try_borrow_mut())
    else {
        return Err(Error::InvalidState);
    };
    if attaches.held.saturating_add(attaches.pending.len()) >= registration_limit(set) {
        return Err(Error::ResourceLimit);
    }
    let id = attaches.take_id()?;
    let timer = hypha_core::timer::Timer::new(framework_period, clock_now(sim_time));
    let control = Some(TimerControl::new(&timer, sim_time));
    // Nothing refuses the attach from here: the C context changes owner.
    let callback = Some(callback.adopt());
    let target = Target::Timer { timer, callback, cycle: None, run_anchored: true, control };
    let registration = Registration::new(id, target);
    live.push(registration.live_entry());
    attaches.pending.push(registration);
    Ok(id)
}

/// Attach `handler` for each sample `subscriber` takes, inside a run. The registration
/// owns both until the detach or the destruction of the `WaitSet`.
///
/// # Errors
/// `InvalidState` during a run or for a subscriber another registration dispatches,
/// `InvalidOperation` for one an owning receive claimed, `ResourceLimit` at the
/// registration limit, and the transport's error otherwise.
pub fn waitset_attach_subscription(
    set: &HyphaWaitSet,
    subscriber: Arc<HyphaSubscriber>,
    handler: Box<dyn SubscriptionHandler>,
) -> Result<HyphaAttachmentId, Error> {
    let callback = SubscriptionCallback::Native(handler);
    attach(set, Request::Subscription { subscriber, callback })
}

fn register(set: &HyphaWaitSet, request: Request) -> Result<(HyphaAttachmentId, Target), Error> {
    match request {
        Request::Subscription { subscriber, callback } => {
            let previous = subscriber.attach_for_dispatch()?;
            // A wildcard subscription has no event service, so it is read on the poll period.
            let attached =
                subscriber.port.open_listener().map_err(Error::from).and_then(|listener| {
                    let trigger = if listener.is_some() {
                        Trigger::Event
                    } else {
                        Trigger::Periodic(WILDCARD_POLL_PERIOD)
                    };
                    set.native_attach(listener, trigger)
                });
            match attached {
                Ok(id) => {
                    // Nothing refuses the attach from here: the C context changes owner.
                    let callback = match callback {
                        SubscriptionCallback::Native(handler) => DataCallback::Native(handler),
                        SubscriptionCallback::Foreign(foreign) => {
                            DataCallback::Foreign(Some(foreign.adopt()))
                        }
                    };
                    Ok((id, Target::Subscription { subscriber, callback, head: None }))
                }
                Err(status) => {
                    // A refused attach claims nothing.
                    subscriber.undo_attach(previous);
                    Err(status)
                }
            }
        }
        Request::Timer { period, framework_period, callback, cycle } => {
            let node_clock = set.node().uses_sim_time;
            let sim_time = set.node().on_sim_time();
            let wake = timer_wake(period, node_clock, sim_time);
            let id = set.native_attach(None, wake)?;
            // A cycle on simulated time keeps its deadlines on the epoch grid.
            let timer = match (cycle.is_some() && sim_time, hypha_time::sim_time_has_value()) {
                (true, true) => {
                    hypha_core::timer::Timer::on_grid(framework_period, hypha_time::now())
                }
                (true, false) => hypha_core::timer::Timer::awaiting_grid(framework_period),
                (false, _) => hypha_core::timer::Timer::new(framework_period, clock_now(sim_time)),
            };
            // A plain timer can be canceled and reset; a cycle runs at its rate.
            let control = cycle.is_none().then(|| TimerControl::new(&timer, sim_time));
            let cycle = cycle.map(super::replay::CycleTimer::new);
            // Nothing refuses the attach from here: the C context changes owner.
            let callback = callback.map(Adoption::adopt);
            // A plain timer made before the first clock value starts on that value, not on
            // the placeholder zero.
            let run_anchored = sim_time && (cycle.is_some() || hypha_time::sim_time_has_value());
            Ok((id, Target::Timer { timer, callback, cycle, run_anchored, control }))
        }
        Request::Triggered { cycle, silence_nanos, notified } => {
            let schedule = hypha_runtime::trigger_schedule::TriggerSchedule::new(silence_nanos)
                .ok_or(Error::InvalidArgument)?;
            let wake = triggered_wake(set, silence_nanos, notified);
            let id = set.native_attach(None, wake)?;
            let state = super::triggered::TriggeredCycle::new(cycle, schedule);
            Ok((id, Target::Triggered(state)))
        }
        Request::Driver(driver) => {
            let wake = Trigger::Periodic(super::clock_driver::ACK_POLL_INTERVAL);
            let id = set.native_attach(None, wake)?;
            Ok((id, Target::Driver(driver)))
        }
        Request::TriggerWake { listener } => {
            let id = set.native_attach(Some(*listener), Trigger::Event)?;
            Ok((id, Target::TriggerWake))
        }
    }
}

/// The wake of a framework timer.
///
/// Host time has no wake period of its own, and a node on its own simulated clock is
/// woken by its `/clock` listener; a clock driven from outside is polled at 1 ms.
fn timer_wake(period: Duration, node_clock: bool, sim_time: bool) -> Trigger {
    if !sim_time || node_clock {
        return Trigger::Timed;
    }
    Trigger::Periodic(period.min(Duration::from_millis(1)))
}

/// The wake of a triggered cycle. An unnotified cycle has no listener and polls its
/// triggers at a tenth of its limit.
fn triggered_wake(set: &HyphaWaitSet, silence_nanos: i64, notified: bool) -> Trigger {
    // A node on its own simulated clock is woken by its `/clock` listener.
    if set.node().uses_sim_time {
        return Trigger::Timed;
    }
    if hypha_time::is_sim_time() {
        return Trigger::Periodic(Duration::from_millis(1));
    }
    if notified {
        return Trigger::Timed;
    }
    let tenth = u64::try_from(silence_nanos / 10).unwrap_or(0);
    Trigger::Periodic(Duration::from_nanos(tenth).max(WILDCARD_POLL_PERIOD))
}

/// Attach a committed triggered cycle: its periodic wake, and `listener` on its trigger
/// topic when there is one. Returns both registrations, zero without a listener. On
/// failure nothing stays attached.
pub(crate) fn attach_triggered_cycle(
    set: &HyphaWaitSet,
    cycle: &Cycle,
    silence_nanos: i64,
    listener: Option<Box<hypha_iceoryx2::Iox2Listener>>,
) -> Result<(HyphaAttachmentId, HyphaAttachmentId), Error> {
    let request =
        Request::Triggered { cycle: cycle.id, silence_nanos, notified: listener.is_some() };
    let wake = attach(set, request)?;
    let Some(listener) = listener else {
        return Ok((wake, 0));
    };
    attach(set, Request::TriggerWake { listener }).map(|id| (wake, id)).inspect_err(|_| {
        let _ = waitset_detach(set, wake);
    })
}

/// Attach the framework-time timer of `cycle`, a cycle of `set`. The cycle is its owner.
pub(crate) fn attach_cycle_timer(
    set: &HyphaWaitSet,
    cycle: &Cycle,
) -> Result<HyphaAttachmentId, Error> {
    let period_nanos = u64::try_from(cycle.period_nanos()).unwrap_or(0);
    let request = Request::timer(period_nanos, None, Some(cycle.id))?;
    attach(set, request)
}

/// Remove the registration `attachment` of `set` and release what it owns; from a
/// callback of the running `WaitSet`, defer it.
///
/// # Errors
/// `InvalidArgument` for an unknown registration; `InvalidState` from another thread
/// during a run.
pub fn waitset_detach(set: &HyphaWaitSet, attachment: HyphaAttachmentId) -> Result<(), Error> {
    if set.is_running() {
        return defer_detach(set, attachment);
    }
    remove_registration(set, attachment)
}

/// A detach during a run: accepted only from the run's own thread, a callback.
fn defer_detach(set: &HyphaWaitSet, attachment: HyphaAttachmentId) -> Result<(), Error> {
    let from_the_run = set.shared.run_owner() == Some(std::thread::current().id());
    if !from_the_run {
        return Err(Error::InvalidState);
    }
    // The dispatcher holds the registrations for the run, so the id is checked against
    // the registry of live registrations.
    let Ok(mut pending) = set.pending_detach.try_borrow_mut() else {
        return Err(Error::InvalidState);
    };
    let known = set
        .live
        .try_borrow()
        .is_ok_and(|live| live.iter().any(|registration| registration.id == attachment));
    if !known {
        return Err(Error::InvalidArgument);
    }
    if !pending.contains(&attachment) {
        pending.push(attachment);
    }
    Ok(())
}

pub(crate) fn finish_deferred_detaches(set: &HyphaWaitSet) {
    let pending = match set.pending_detach.try_borrow_mut() {
        Ok(mut pending) => std::mem::take(&mut *pending),
        Err(_) => return,
    };
    for attachment in pending {
        let removed = remove_registration(set, attachment);
        // A release that ran earlier in this loop may have removed it already.
        let unexpected = !matches!(removed, Ok(()) | Err(Error::InvalidArgument));
        if unexpected {
            let status = crate::status::status_of(removed);
            log::error!(target: "hypha", "hypha: attachment {attachment} detached by a callback could not be removed: {status:?}");
        }
    }
}

/// `InvalidArgument` for an unknown registration.
fn remove_registration(set: &HyphaWaitSet, attachment: HyphaAttachmentId) -> Result<(), Error> {
    // The registration leaves the registry, and the borrows end, before its context
    // is released: a release that reaches this `WaitSet` again finds it consistent.
    let mut registration = {
        let (Ok(mut registrations), Ok(mut native)) =
            (set.registrations.try_borrow_mut(), set.native.try_borrow_mut())
        else {
            return Err(Error::InvalidState);
        };
        let Some(index) =
            registrations.iter().position(|registration| registration.id == attachment)
        else {
            return Err(Error::InvalidArgument);
        };
        // A timer attached during a run has no native attachment.
        let native_registration = attachment <= LAST_NATIVE_IDENTIFIER;
        if native_registration && native.detach(attachment).is_err() {
            return Err(Error::InvalidArgument);
        }
        registrations.remove(index)
    };
    if let Ok(mut live) = set.live.try_borrow_mut() {
        live.retain(|registration| registration.id != attachment);
    }
    registration.end_dispatch();
    registration.release_context();
    Ok(())
}

/// Attaches the registration a C caller described in `attachment` to the `WaitSet` that
/// `reach` returns. Every field is checked before `share` runs; a refused attach leaves
/// the context to the caller.
///
/// # Errors
/// `InvalidArgument` for an unknown kind, a missing callback or subscriber, or a field
/// the kind does not use; the error of `reach`; an attach error.
pub fn waitset_attach_foreign<'set, C: ForeignCallbacks>(
    reach: impl FnOnce() -> Result<&'set HyphaWaitSet, Error>,
    attachment: &HyphaAttachment,
    share: impl FnOnce(*mut HyphaSubscriber) -> Arc<HyphaSubscriber>,
) -> Result<HyphaAttachmentId, Error> {
    let request = Request::decode::<C>(attachment, share)?;
    attach(reach()?, request)
}
