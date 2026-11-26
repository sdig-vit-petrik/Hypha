//! Triggered cycles: one step per trigger sample, and a fallback step when the trigger
//! falls silent. The schedule rules are in `hypha_runtime::trigger_schedule`.

use hypha_runtime::trigger_schedule::{TriggerEvent, TriggerSchedule};
use hypha_runtime::upstream_hold::Limit;

use super::upstream::UpstreamHold;
use crate::cycle::{Cycle, CycleId};
use crate::status::Error;

/// The registration state of one triggered cycle. Only the dispatcher reads it.
pub(super) struct TriggeredCycle {
    pub(super) cycle: CycleId,
    pub(super) schedule: TriggerSchedule,
    pub(super) upstream: UpstreamHold,
    /// The stamps of each trigger port still pending after the last turn, for the
    /// progress report.
    pub(super) pending: Vec<Vec<i64>>,
    /// The upstream limit of the last turn, for the progress report.
    pub(super) limit: Limit,
    /// Scratch for the events of one turn.
    pub(super) events: Vec<TriggerEvent>,
    /// The clock of the last turn, to see a rewind.
    pub(super) last_now: Option<i64>,
}

impl TriggeredCycle {
    /// The state of `cycle`, whose trigger falls silent after `schedule`'s limit.
    pub(super) fn new(cycle: CycleId, schedule: TriggerSchedule) -> Box<Self> {
        Box::new(Self {
            cycle,
            schedule,
            upstream: UpstreamHold::default(),
            pending: vec![Vec::new()],
            limit: Limit { at: None, blocking: None },
            events: Vec::new(),
            last_now: None,
        })
    }
}

use hypha_runtime::trigger_schedule::{TriggerAdmit, TriggerClock};

/// The state of the triggered cycle `attachment`; its absence is an invalid state.
fn triggered_state(
    registrations: &mut [Registration],
    attachment: AttachmentId,
) -> Result<&mut TriggeredCycle, Error> {
    registrations
        .iter_mut()
        .find(|registration| registration.id == attachment)
        .and_then(|registration| match registration.target {
            Target::Triggered(ref mut state) => Some(&mut **state),
            _ => None,
        })
        .ok_or(Error::InvalidState)
}

use super::dispatch::{CallbackDispatcher, fail_with, record_action};
use super::{AttachmentId, Progression, Registration, Target, invoke_cycle};

impl CallbackDispatcher<'_> {
    /// Run the due steps of the triggered cycle `attachment` at `now`: one per trigger
    /// sample, oldest first, and a fallback for each silence limit without one.
    ///
    /// Under simulated time a step at `t` runs once the clock reached `t` and every
    /// upstream reported at least `t - 1`, and its other inputs are cut strictly before `t`.
    pub(super) fn run_triggered(&mut self, attachment: AttachmentId, now: i64) -> Progression {
        self.triggered_turn(attachment, now)
            .unwrap_or_else(|status| fail_with(&mut self.failure, attachment, status))
    }

    fn triggered_turn(&mut self, attachment: AttachmentId, now: i64) -> Result<Progression, Error> {
        let sim = self.node.on_sim_time();
        if sim && !hypha_time::sim_time_has_value() {
            // Nothing runs before the first clock value, as for a grid timer.
            return Ok(Progression::Continue);
        }
        // On host time the turn takes its own steady and wall readings, together: a trigger
        // stamp is mapped from the wall clock onto the steady one by their difference.
        let (now, wall) = if sim {
            (now, 0)
        } else {
            (hypha_time::steady_now().nanos(), hypha_time::wall_now().nanos())
        };
        let node = self.node;
        let health = self.health;
        let set = self.set;
        let detached = self.detached;
        let id = triggered_state(self.registrations, attachment)?.cycle;
        let cycle: &Cycle = self.cycle(id)?;
        // The steps reach the dispatcher's other fields directly, never the dispatcher as a whole.
        let state = triggered_state(self.registrations, attachment)?;
        let trigger = cycle.step_ref()?.runtime.trigger().ok_or(Error::InvalidState)?;
        let rewound = state.last_now.is_some_and(|last| now < last);
        match state.last_now {
            None => {
                let clock = u64::try_from(now).map_err(|_| Error::InvalidState)?;
                cycle.step_mut()?.runtime.start_silence(trigger, clock);
            }
            Some(_) if rewound => {
                let subject = format!("cycle '{}'", cycle.name());
                return Err(Error::from_status(crate::replay_restart::refuse_rewind(
                    &subject, now,
                )));
            }
            Some(_) => {}
        }
        state.last_now = Some(now);
        state.limit = if sim {
            state.upstream.settle(&cycle.step_ref()?.inputs, node)?
        } else {
            hypha_runtime::upstream_hold::Limit { at: None, blocking: None }
        };
        crate::cycle::drain_inputs(cycle)?;
        pending_stamps(cycle, &mut state.pending)?;
        let clock = if sim {
            TriggerClock::Sim { now, limit: &state.limit }
        } else {
            TriggerClock::Host { wall, steady: now }
        };
        let admitted = state.schedule.admit(&state.pending, clock, &mut state.events);
        match admitted.map_err(Error::from)? {
            TriggerAdmit::Run => {}
            TriggerAdmit::OutOfOrder { stamp, before } => {
                log::error!(target: "hypha", "hypha: cycle '{}' has a trigger stamped {stamp} ns after a step at {before} ns: under simulated time the trigger stamps must ascend, and each publisher must report its progress", cycle.name());
                return Err(Error::SequenceGap);
            }
        }
        let mut decision = Progression::Continue;
        for &event in &state.events {
            // A step may detach its own cycle's registration: no further step then.
            let gone = detached.try_borrow().is_ok_and(|ids| ids.contains(&attachment));
            if gone {
                break;
            }
            let (time, take) = match event {
                TriggerEvent::Trigger { time, .. } => (time, 1),
                TriggerEvent::Fallback { time, .. } => (time, 0),
                TriggerEvent::Late { .. } => {
                    // Stamped before a step that ran, or older than the limit: consumed
                    // and counted as an age rejection, with no step.
                    cycle.step_mut()?.runtime.refuse_trigger().map_err(Error::from_code)?;
                    continue;
                }
            };
            {
                let mut step = cycle.step_mut()?;
                step.scheduled_nanos = time;
                step.input_cut = sim.then_some(time);
                step.trigger_take = Some(take);
            }
            let result = invoke_cycle(health, cycle, set, 0);
            // Clearing them keeps a step that never ran from leaking them.
            if let Ok(mut step) = cycle.step_mut() {
                step.input_cut = None;
                step.trigger_take = None;
            }
            decision = record_action(node, &mut self.failure, attachment, result);
            if decision != Progression::Continue {
                break;
            }
        }
        pending_stamps(cycle, &mut state.pending)?;
        Ok(decision)
    }
}

/// Replace `pending` with the pending stamps of the trigger of `cycle`.
fn pending_stamps(cycle: &Cycle, pending: &mut [Vec<i64>]) -> Result<(), Error> {
    let stamps = pending.first_mut().ok_or(Error::InvalidState)?;
    cycle.step_mut()?.runtime.pending_trigger_stamps(stamps).map_err(Error::from_code)
}
