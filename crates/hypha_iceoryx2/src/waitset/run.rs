//! Native guard lifetimes and bounded dispatch turns.

use super::{
    Dispatcher, EventOutcome, IDLE_BACKSTOP, Iox2WaitSet, Ipc, Progression, Registration, Trigger,
    TurnState,
};
use core::sync::atomic::Ordering;
use core::time::Duration;
use hypha_core::port::PortError;
use hypha_core::port::dispatch::{AttachmentId, RunReport, StopReason};
use iceoryx2::port::listener::Listener;
use iceoryx2::prelude::CallbackProgression;
use iceoryx2::waitset::{WaitSetBuilder, WaitSetRunResult};

impl Iox2WaitSet {
    pub(super) fn run_bounded(
        &mut self,
        budget: u32,
        deadline: Option<std::time::Instant>,
        dispatcher: &mut impl Dispatcher,
    ) -> RunReport {
        // Cleared when this run returns, never at entry: clearing here would lose a
        // stop that arrived while the run was starting.
        let mut report = self.run_inner(budget, deadline, dispatcher);
        // A stop that arrived after the run last looked is reported, never cleared unreported.
        let stop_pending = self.stop.swap(false, Ordering::SeqCst);
        if stop_pending && report.reason == StopReason::Deadline {
            report = RunReport::requested();
        }
        report
    }

    fn run_inner(
        &mut self,
        budget: u32,
        deadline: Option<std::time::Instant>,
        dispatcher: &mut impl Dispatcher,
    ) -> RunReport {
        // The application owns SIGINT and SIGTERM; the iceoryx2 default would replace them.
        let Ok(waitset) = WaitSetBuilder::new()
            .signal_handling_mode(iceoryx2::prelude::SignalHandlingMode::Disabled)
            .create::<Ipc>()
        else {
            return RunReport::setup_failed(PortError::Backend(32));
        };

        // A guard borrows a listener for the whole run.
        let listeners = &self.listeners;
        let registrations = &mut self.registrations;

        let Ok(control_guard) = waitset.attach_notification(&self.control.listener) else {
            return RunReport::setup_failed(PortError::Backend(32));
        };

        let (mut event_guards, mut interval_guards) = (Vec::new(), Vec::new());
        if let Err(report) = build_guards(
            &waitset,
            listeners,
            registrations,
            &mut event_guards,
            &mut interval_guards,
        ) {
            // Dropping the guards detaches them in reverse order.
            return report;
        }

        let count = registrations.len();
        let mut state = TurnState::new(count);
        let mut cursor = 0_usize;
        let mut first_turn = true;

        loop {
            if self.stop.load(Ordering::SeqCst) {
                return RunReport::requested();
            }
            let has_pending_work = state.has_pending_work();
            let first = std::mem::take(&mut first_turn);
            let Some(wait_slice) = next_wait_slice(deadline, has_pending_work, first) else {
                return RunReport::deadline();
            };
            let wait_slice = match dispatcher.next_timed_wait() {
                Some(timed_wait) => wait_slice.min(timed_wait),
                None => wait_slice,
            };

            let mut stop_requested = false;
            let wait = waitset.wait_and_process_once_with_timeout(
                |attachment| {
                    if attachment.has_event_from(&control_guard) {
                        stop_requested = true;
                    }
                    for (index, guard) in &event_guards {
                        if attachment.has_event_from(guard)
                            && let Some(slot) = state.ready.get_mut(*index)
                        {
                            *slot = true;
                        }
                    }
                    for (index, guard) in &interval_guards {
                        if attachment.has_event_from(guard)
                            && let Some(slot) = state.due.get_mut(*index)
                        {
                            *slot = slot.saturating_add(1);
                        }
                    }
                    CallbackProgression::Continue
                },
                wait_slice,
            );

            match wait {
                Ok(WaitSetRunResult::AllEventsHandled) => {}
                // A non-stop signal interrupts the wait; the stop flags are checked next turn.
                Ok(WaitSetRunResult::TerminationRequest | WaitSetRunResult::Interrupt) => continue,
                Ok(WaitSetRunResult::StopRequest) => return RunReport::requested(),
                Err(_) => return RunReport::runtime_failed(PortError::Backend(34), None),
            }

            if stop_requested && let Err(cause) = drain(&self.control.listener) {
                return RunReport::runtime_failed(cause, None);
            }

            if let Some(report) =
                mark_ready(listeners, registrations, &event_guards, &interval_guards, &mut state)
            {
                return report;
            }

            // Timers first, so a sample one publishes goes out this turn.
            if let Some(report) = terminal(dispatcher.on_timers(), 0) {
                return report;
            }

            if let Some(report) =
                dispatch_turn(registrations, &mut state, &mut cursor, budget, dispatcher)
            {
                return report;
            }

            // Expiry is checked after every turn, so invalid traffic cannot keep a stale value usable.
            if let Some(report) = terminal(dispatcher.on_tick(), 0) {
                return report;
            }

            if self.stop.load(Ordering::SeqCst) {
                return RunReport::requested();
            }
        }
    }
}

/// How long the next native wait blocks, or `None` when the deadline elapsed.
///
/// The first turn always happens without blocking: setup on a loaded host can
/// outlast a short limit.
fn next_wait_slice(
    deadline: Option<std::time::Instant>,
    has_pending_work: bool,
    first_turn: bool,
) -> Option<Duration> {
    let slice = if has_pending_work { Duration::ZERO } else { IDLE_BACKSTOP };
    let Some(deadline) = deadline else {
        return Some(slice);
    };
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return first_turn.then_some(Duration::ZERO);
    }
    Some(slice.min(remaining))
}

type EventGuards<'w, 'l> = Vec<(usize, iceoryx2::waitset::WaitSetGuard<'w, 'l, Ipc>)>;

/// Native interval guards; `'static`, so apart from the event guards.
type IntervalGuards<'w> = Vec<(usize, iceoryx2::waitset::WaitSetGuard<'w, 'static, Ipc>)>;

/// Create every native attachment for one run, before any callback runs.
fn build_guards<'w, 'l>(
    waitset: &'w iceoryx2::waitset::WaitSet<Ipc>,
    listeners: &'l [Vec<Listener<Ipc>>],
    registrations: &[Registration],
    event_guards: &mut EventGuards<'w, 'l>,
    interval_guards: &mut IntervalGuards<'w>,
) -> Result<(), RunReport> {
    for (index, registration) in registrations.iter().enumerate() {
        if registration.trigger == Trigger::Timed {
            continue;
        }
        let attached = match (registration.trigger, listeners.get(index)) {
            (Trigger::Event, Some(listeners)) if !listeners.is_empty() => {
                listeners.iter().try_for_each(|listener| {
                    waitset
                        .attach_notification(listener)
                        .map(|guard| event_guards.push((index, guard)))
                })
            }
            (Trigger::Periodic(period), _) => {
                waitset.attach_interval(period).map(|guard| interval_guards.push((index, guard)))
            }
            _ => return Err(RunReport::setup_failed(PortError::InvalidTopic)),
        };
        if attached.is_err() {
            return Err(RunReport::setup_failed(PortError::Backend(33)));
        }
    }
    Ok(())
}

/// Mark every attachment that has work, and consume its notifications.
fn mark_ready(
    listeners: &[Vec<Listener<Ipc>>],
    registrations: &mut [Registration],
    event_guards: &EventGuards<'_, '_>,
    interval_guards: &IntervalGuards<'_>,
    state: &mut TurnState,
) -> Option<RunReport> {
    for (index, _) in event_guards {
        if let Some(report) = mark_event_ready(listeners, registrations, state, *index) {
            return Some(report);
        }
    }
    for (index, _) in interval_guards {
        mark_interval_ready(state, *index);
    }
    None
}

/// Consume a notified registration's notifications before the dispatcher reads, so an
/// unread event cannot spin the run loop.
fn mark_event_ready(
    listeners: &[Vec<Listener<Ipc>>],
    registrations: &[Registration],
    state: &TurnState,
    index: usize,
) -> Option<RunReport> {
    if !state.ready.get(index).copied().unwrap_or(false) {
        return None;
    }
    let registration = registrations.get(index)?;
    let drained = listeners.get(index)?.iter().try_for_each(|listener| drain(listener).map(drop));
    drained.err().map(|cause| RunReport::runtime_failed(cause, Some(registration.id)))
}

/// A periodic registration is ready for exactly one turn per tick.
fn mark_interval_ready(state: &mut TurnState, index: usize) {
    if state.due.get(index).copied().unwrap_or(0) == 0 {
        return;
    }
    if let Some(slot) = state.ready.get_mut(index) {
        *slot = true;
    }
}

/// Deliver one bounded turn of work, rotating over the ready registrations so a
/// busy one cannot starve the others.
fn dispatch_turn(
    registrations: &mut [Registration],
    state: &mut TurnState,
    cursor: &mut usize,
    budget: u32,
    dispatcher: &mut impl Dispatcher,
) -> Option<RunReport> {
    let count = registrations.len();
    if count == 0 {
        return None;
    }

    let mut spent = 0_u32;
    let mut examined = 0_usize;
    while spent < budget && examined < count {
        let index = cursor.checked_rem(count)?;
        *cursor = index.wrapping_add(1).checked_rem(count)?;
        examined = examined.saturating_add(1);

        let Some(registration) = registrations.get_mut(index) else {
            continue;
        };
        if !state.ready.get(index).copied().unwrap_or(false) {
            continue;
        }
        examined = 0;
        spent = spent.saturating_add(1);

        let missed = state.due.get(index).copied().unwrap_or(0);
        if missed > 0 {
            if let Some(slot) = state.due.get_mut(index) {
                *slot = 0;
            }
            let answer = dispatcher.on_interval(registration.id, missed.saturating_sub(1));
            if let Some(report) = terminal(answer, registration.id) {
                return Some(report);
            }
        }

        match dispatcher.on_event(registration.id) {
            EventOutcome::Drained => {
                if let Some(slot) = state.ready.get_mut(index) {
                    *slot = false;
                }
            }
            EventOutcome::MoreWork => {}
            EventOutcome::Stop => return Some(RunReport::requested()),
            EventOutcome::Fail(cause) => {
                return Some(RunReport::runtime_failed(cause, Some(registration.id)));
            }
        }
    }
    None
}

/// Turn a dispatcher answer into a run report, or `None` to keep running.
fn terminal(progression: Progression, attachment: AttachmentId) -> Option<RunReport> {
    match progression {
        Progression::Continue => None,
        Progression::Stop => Some(RunReport::requested()),
        Progression::Fail(cause) => Some(RunReport::runtime_failed(cause, Some(attachment))),
    }
}

/// Consume pending notifications so an unread event cannot spin the wait loop.
fn drain(listener: &Listener<Ipc>) -> Result<u64, PortError> {
    listener.try_wait(|_| {}).map_err(|cause| {
        log::error!(target: "hypha/transport", "hypha_iceoryx2: failed to drain a notification listener: {cause:?}");
        PortError::Disconnected
    })
}
