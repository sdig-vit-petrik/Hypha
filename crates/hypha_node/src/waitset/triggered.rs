//! Triggered cycles: one step per sample of any trigger port, and a fallback step when the
//! triggers fall silent. The schedule rules are in `hypha_runtime::trigger_schedule`.

use hypha_cycle::TriggerStep;
use hypha_runtime::trigger_schedule::{TriggerAdmit, TriggerClock, TriggerEvent, TriggerSchedule};
use hypha_runtime::upstream_hold::Limit;

use super::dispatch::{CallbackDispatcher, fail_with, record_action};
use super::upstream::UpstreamHold;
use super::{AttachmentId, ExecutionHealth, Progression, Registration, Target, invoke_cycle};
use crate::cycle::{Cycle, CycleId};
use crate::status::Error;
use crate::waitset::{HyphaCallbackResult, HyphaWaitSet};

/// The registration state of one triggered cycle. Only the dispatcher reads it.
pub(super) struct TriggeredCycle {
    pub(super) cycle: CycleId,
    pub(super) schedule: TriggerSchedule,
    pub(super) upstream: UpstreamHold,
    /// The stamps each trigger port still holds after the last turn, for the progress
    /// report.
    pub(super) pending: Vec<Vec<i64>>,
    /// The upstream limit of the last turn, for the progress report.
    pub(super) limit: Limit,
    /// Scratch for the events of one turn.
    pub(super) events: Vec<TriggerEvent>,
    /// The clock of the last turn, to see a rewind.
    pub(super) last_now: Option<i64>,
}

impl TriggeredCycle {
    /// The state of `cycle`, whose trigger ports fall silent after `schedule`'s limits.
    pub(super) fn new(cycle: CycleId, schedule: TriggerSchedule) -> Box<Self> {
        Box::new(Self {
            cycle,
            schedule,
            upstream: UpstreamHold::default(),
            pending: Vec::new(),
            limit: Limit { at: None, blocking: None },
            events: Vec::new(),
            last_now: None,
        })
    }

    /// Take the clock of a turn: the first evaluation starts the silence of every trigger
    /// port, and a clock that went back ends the run.
    fn observe(&mut self, cycle: &Cycle, now: i64) -> Result<(), Error> {
        match self.last_now {
            None => {
                let clock = u64::try_from(now).map_err(|_| Error::InvalidState)?;
                cycle.step_mut()?.runtime.start_trigger_silence(clock);
            }
            Some(last) if now < last => {
                let subject = format!("cycle '{}'", cycle.name());
                let rewound = crate::replay_restart::refuse_rewind(&subject, now);
                return Err(Error::from_status(rewound));
            }
            Some(_) => {}
        }
        self.last_now = Some(now);
        Ok(())
    }

    /// Read the stamps each trigger port of `cycle` holds.
    fn read_pending(&mut self, cycle: &Cycle) -> Result<(), Error> {
        let step = cycle.step_ref()?;
        step.runtime.pending_trigger_stamps(&mut self.pending).map_err(Error::from_code)
    }

    /// Admit the events of this turn on `clock` into `events`.
    fn admit(&mut self, cycle: &Cycle, clock: TriggerClock<'_>) -> Result<(), Error> {
        match self.schedule.admit(&self.pending, clock, &mut self.events)? {
            TriggerAdmit::Run => Ok(()),
            TriggerAdmit::OutOfOrder { stamp, before } => {
                log::error!(target: "hypha", "hypha: cycle '{}' has a trigger stamped {stamp} ns after a step at {before} ns: under simulated time the trigger stamps of each port must ascend, and each publisher must report its progress", cycle.name());
                Err(Error::SequenceGap)
            }
        }
    }
}

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

impl CallbackDispatcher<'_> {
    /// Run the due steps of the triggered cycle `attachment` at `now`: one per sample of
    /// any trigger port, in merge order, and the fallbacks due before them.
    ///
    /// Under simulated time a step at `t` runs once the clock reached `t` and every
    /// upstream reported at least `t`, and its other inputs are cut strictly before `t`.
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
        state.observe(cycle, now)?;
        let limit = if sim {
            state.upstream.settle(&cycle.step_ref()?.inputs, node)?
        } else {
            Limit { at: None, blocking: None }
        };
        crate::cycle::drain_inputs(cycle)?;
        state.read_pending(cycle)?;
        let clock = if sim {
            TriggerClock::Sim { now, limit: &limit }
        } else {
            TriggerClock::Host { wall, steady: now }
        };
        state.admit(cycle, clock)?;
        state.limit = limit;
        let mut decision = Progression::Continue;
        for &event in &state.events {
            // A step may detach its own cycle's registration: no further step then.
            let gone = detached.try_borrow().is_ok_and(|ids| ids.contains(&attachment));
            if gone {
                break;
            }
            let (time, trigger) = match event {
                TriggerEvent::Trigger { time, port } => (time, TriggerStep::Sample { port }),
                TriggerEvent::Fallback { time, due } => (time, TriggerStep::Fallback { due }),
                TriggerEvent::Late { port, .. } => {
                    // Older than its port's limit, or stamped before its port's last step:
                    // consumed and counted as an age rejection of that port, with no step.
                    cycle.step_mut()?.runtime.refuse_trigger(port).map_err(Error::from_code)?;
                    continue;
                }
            };
            let result = step_triggered(cycle, set, health, time, trigger, sim)?;
            decision = record_action(node, &mut self.failure, attachment, result);
            if decision != Progression::Continue {
                break;
            }
        }
        state.read_pending(cycle)?;
        Ok(decision)
    }
}

/// Run one step of `cycle` at `time`, its trigger ports holding `trigger`; under simulated
/// time its other inputs are cut at `time`.
fn step_triggered(
    cycle: &Cycle,
    set: &HyphaWaitSet,
    health: &ExecutionHealth,
    time: i64,
    trigger: TriggerStep,
    sim: bool,
) -> Result<HyphaCallbackResult, Error> {
    {
        let mut step = cycle.step_mut()?;
        step.scheduled_nanos = time;
        step.input_cut = sim.then_some(time);
        step.trigger_step = Some(trigger);
    }
    let result = invoke_cycle(health, cycle, set, 0);
    // Clearing them keeps a step that never ran from leaking them.
    if let Ok(mut step) = cycle.step_mut() {
        step.input_cut = None;
        step.trigger_step = None;
    }
    Ok(result)
}
