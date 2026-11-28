//! When a timer registration is due: the host-clock timers, a cycle's replay grid, and the
//! rewind that ends the run (FSR-37).

use hypha_runtime::timer_schedule::{callback_deadline, replay_periods};

use super::super::replay::{CycleTimer, ReplayDue};
use super::super::{AttachmentId, HyphaNode, HyphaStatus, Target};
use super::{Due, find_cycle};
use crate::cycle::Cycle;

/// Start a host timer's periods at the first run turn, not at the attach, and return
/// the deadline it is due at now.
fn anchor_at_first_run(
    timer: &mut hypha_core::timer::Timer,
    run_anchored: &mut bool,
    now: hypha_time::Time,
) -> i64 {
    if !*run_anchored {
        timer.reset(now);
        *run_anchored = true;
    }
    timer.next_due().nanos()
}

/// Observe `timer` at `now`, with the periods this observation skipped: the `missed` of
/// the callback, not the running total.
fn observe_with_delta(
    timer: &mut hypha_core::timer::Timer,
    now: hypha_time::Time,
) -> (hypha_core::timer::Observation, u64) {
    let previous_missed = timer.missed_count();
    let observation = timer.observe(now);
    (observation, timer.missed_count().wrapping_sub(previous_missed))
}

/// What the timer of registration `id` is due for at `now`: the replay call of a cycle's
/// grid timer, or one call of a timer on the node's clock. `None` when it has nothing to
/// run, or when `target` is no timer.
///
/// # Errors
/// The status that ends the run: a rewind of simulated time (FSR-37), an unknown cycle,
/// or a deadline outside the range of the clock.
pub(super) fn timer_due(
    cycles: &[Cycle],
    node: &HyphaNode,
    id: AttachmentId,
    target: &mut Target,
    now: hypha_time::Time,
) -> Result<Option<Due>, HyphaStatus> {
    let Target::Timer { ref mut timer, ref mut cycle, ref mut run_anchored, ref control, .. } =
        *target
    else {
        return Ok(None);
    };
    if timer.is_grid()
        && let Some(owned) = cycle.as_mut()
    {
        return grid_due(cycles, id, timer, owned, now);
    }
    // On simulated time a plain timer starts on the first clock value, not on the
    // placeholder zero a node starts with.
    if !*run_anchored && node.on_sim_time() && !hypha_time::sim_time_has_value() {
        return Ok(None);
    }
    if let Some(control) = control.as_deref() {
        // A reset or a canceled timer that finds time gone back is a rewind, as for any
        // timer: the run ends and the process restarts (FSR-37).
        let followed = control.apply_reset(timer, run_anchored)
            && (!control.is_canceled()
                || super::super::TimerControl::follow_while_canceled(timer, now));
        if !followed {
            return Err(refuse_timer_rewind(None, id, timer, now));
        }
        // A canceled timer calls nothing and counts no missed period.
        if control.is_canceled() {
            return Ok(None);
        }
    }
    let scheduled = anchor_at_first_run(timer, run_anchored, now);
    let (hypha_core::timer::Observation { fired, rewound }, missed_delta) =
        observe_with_delta(timer, now);
    if let Some(control) = control.as_deref() {
        control.observed(timer.next_due());
    }
    let owner = cycle.as_ref().map(|owned| find_cycle(cycles, owned.cycle)).transpose()?;
    if rewound {
        return Err(refuse_timer_rewind(owner, id, timer, now));
    }
    if fired == 0 {
        return Ok(None);
    }
    // A cycle's step learns the deadline it runs for.
    if let Some(cycle) = owner {
        let deadline = callback_deadline(scheduled, missed_delta, timer.period().nanos())
            .ok_or(HyphaStatus::InvalidState)?;
        cycle.step_mut()?.scheduled_nanos = deadline;
    }
    Ok(Some(Due::Timer {
        attachment: id,
        cycle: owner.map(|cycle| cycle.id),
        missed: missed_delta,
    }))
}

/// The replay call of the grid timer of a cycle at `now`, `None` when it has nothing to
/// run; the status that ends the run for an unknown cycle or a rewind.
fn grid_due(
    cycles: &[Cycle],
    attachment: AttachmentId,
    timer: &mut hypha_core::timer::Timer,
    owned: &mut CycleTimer,
    now: hypha_time::Time,
) -> Result<Option<Due>, HyphaStatus> {
    let CycleTimer { cycle: id, ref mut upstream, ref deferred } = *owned;
    let cycle = find_cycle(cycles, id)?;
    if timer.awaiting_anchor() {
        // Nothing fires before the first clock value sets the phase.
        if !hypha_time::sim_time_has_value() {
            return Ok(None);
        }
        timer.anchor_grid(now);
    }
    let first_nanos = timer.next_due().nanos();
    let (hypha_core::timer::Observation { fired, rewound }, missed_delta) =
        observe_with_delta(timer, now);
    if rewound {
        return Err(refuse_timer_rewind(Some(cycle), attachment, timer, now));
    }
    // A held cycle runs its backlog, and a sliced one the deadlines its last turn did not
    // reach, on a turn with no new deadline.
    let held = upstream.hold.backlog_first().is_some();
    let sliced = deferred.is_some();
    if fired == 0 && !held && !sliced {
        return Ok(None);
    }
    let periods = replay_periods(fired, missed_delta);
    Ok(Some(Due::Replay(ReplayDue {
        attachment,
        cycle: id,
        first_nanos,
        period_nanos: timer.period().nanos(),
        periods,
    })))
}

/// The monotonic deadline of a registration that runs on the host clock, or `None`.
pub(super) fn host_deadline(target: &Target, host_timers: bool) -> Option<i64> {
    match *target {
        Target::Timer { ref timer, ref control, .. } if host_timers => {
            let canceled = control.as_deref().is_some_and(super::super::TimerControl::is_canceled);
            (!canceled).then(|| timer.next_due().nanos())
        }
        Target::Triggered(ref state) if host_timers => state.schedule.next_fallback(),
        _ => None,
    }
}

/// The status that ends the run when the timer of registration `id` saw simulated time
/// go back to `now`. Nothing rewinds in place: the run ends and the process restarts
/// (FSR-37). The line names the cycle `owner` that the timer steps, or the plain timer.
fn refuse_timer_rewind(
    owner: Option<&Cycle>,
    id: AttachmentId,
    timer: &hypha_core::timer::Timer,
    now: hypha_time::Time,
) -> HyphaStatus {
    let subject = owner.map_or_else(
        || timer_subject(id, timer.period().nanos()),
        |cycle| format!("cycle '{}'", cycle.name()),
    );
    crate::replay_restart::refuse_rewind(&subject, now.nanos())
}

/// How a rewind line names a plain timer: by its attachment id and period.
fn timer_subject(id: AttachmentId, period_nanos: i64) -> String {
    let period_millis = period_nanos / 1_000_000;
    format!("timer {id} (period {period_millis} ms)")
}

#[cfg(test)]
mod timer_subject_tests {
    use super::timer_subject;

    #[test]
    fn a_rewound_plain_timer_is_named_by_its_attachment_and_period() {
        assert_eq!(timer_subject(7, 1_000_000_000), "timer 7 (period 1000 ms)");
        assert_eq!(timer_subject(2, 2_500_000), "timer 2 (period 2 ms)");
    }
}

#[cfg(test)]
mod host_deadline_tests {
    use std::sync::Arc;

    use hypha_core::timer::Timer;
    use hypha_time::{Duration, Time};

    use super::host_deadline;
    use crate::waitset::Target;
    use crate::waitset::TimerControl;

    #[test]
    fn a_canceled_host_timer_holds_no_deadline_and_costs_no_wake_up() {
        let timer = Timer::new(Duration::from_nanos(100), Time::from_nanos(1_000));
        let control = TimerControl::new(&timer, false);
        let shared = Some(Arc::clone(&control));
        let target = Target::Timer {
            timer,
            callback: None,
            cycle: None,
            run_anchored: true,
            control: shared,
        };
        assert_eq!(host_deadline(&target, true), Some(1_100));
        control.cancel();
        assert_eq!(host_deadline(&target, true), None);
    }
}
