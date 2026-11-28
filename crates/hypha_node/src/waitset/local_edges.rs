//! The edges between cycles of one `WaitSet` in a replay: a topic that one cycle sends and
//! another reads (#307), and a request and response edge between a client and its server
//! (`docs/design/native-services.md` §4).
//!
//! The cycles of one node report one progress for all of them, so the node-wide upstream
//! hold cannot order them. Within a turn each cycle runs all its due steps before the next
//! cycle runs, so without a hold of its own a reader would run a deadline before the
//! sender ran the steps it reads, and a server before its client ran the step that called
//! it: what a step read would depend on `--chunk` and on the turn order. Each edge holds as
//! an edge between two nodes does: a step at `t` runs once the upstream cycle has run every
//! step before `t` (through `t` for a topic read by a triggered step). The turn repeats its
//! timers while that lets a held step run.

use hypha_runtime::upstream_hold::Limit;

use super::dispatch::{CallbackDispatcher, Due, is_detached_in};
use super::{AttachmentId, Progression, Registration, Target};
use crate::cycle::{Cycle, CycleId, CycleStep};
use crate::status::Error;

/// The most repeats of a turn's timers for the edges between its own cycles. A repeat that
/// runs no step ends them, and so does the turn's step budget of host time
/// ([`super::replay::STEP_SLICE_BUDGET`], shared by every repeat): the held steps left then
/// run on the next turn, which follows at once.
const MAX_REPEATS: u64 = super::replay::MAX_STEPS_PER_TURN;

/// The limits that the other cycles of a cycle's `WaitSet` give its next steps, by the kind
/// of edge: a topic edge needs its upstream through a triggered step's time, a request or
/// response edge one nanosecond less (execution-model.md §6.6 item 5).
pub(super) struct LocalLimits {
    /// Of the cycles that send a topic this cycle reads.
    pub(super) topic: Limit,
    /// Of the cycles that serve this cycle's clients or call its servers.
    pub(super) service: Limit,
}

/// The cycle a registration steps.
fn cycle_of(registration: &Registration) -> Option<CycleId> {
    match registration.target {
        Target::Timer { ref timer, cycle: Some(ref owned), .. } if timer.is_grid() => {
            Some(owned.cycle)
        }
        Target::Triggered(ref state) => Some(state.cycle),
        _ => None,
    }
}

/// True when one of two cycles holds the other: a topic sent by one and read by the other,
/// or a service one serves and the other calls.
fn linked(a: &CycleStep, b: &CycleStep) -> bool {
    a.reads_topic_of(b) || b.reads_topic_of(a) || a.services.is_peer_of(&b.services)
}

/// True when another cycle of `cycles` holds `cycle` or is held by it.
fn has_link(cycles: &[Cycle], cycle: &Cycle) -> bool {
    let Ok(own) = cycle.step_ref() else {
        return false;
    };
    cycles
        .iter()
        .filter(|other| other.id != cycle.id)
        .any(|other| other.step_ref().is_ok_and(|other| linked(&own, &other)))
}

/// Whether `registration` steps a replay or triggered cycle that another cycle of its
/// `WaitSet` holds or is held by.
pub(super) fn is_linked(cycles: &[Cycle], registration: &Registration) -> bool {
    cycle_of(registration)
        .and_then(|id| cycles.iter().find(|cycle| cycle.id == id))
        .is_some_and(|cycle| has_link(cycles, cycle))
}

impl CallbackDispatcher<'_> {
    /// The limits that the other cycles of this `WaitSet` give the next steps of `cycle`:
    /// each upstream's earliest step not run yet, as a topic limit gives it (a deadline up
    /// to it may run). A cycle is no upstream of itself, whatever it reads.
    pub(super) fn local_limits(
        &self,
        attachment: AttachmentId,
        cycle: &Cycle,
    ) -> Result<LocalLimits, Error> {
        let mut limits = LocalLimits {
            topic: Limit { at: None, blocking: None },
            service: Limit { at: None, blocking: None },
        };
        let own = cycle.step_ref()?;
        if own.services.is_empty() && own.inputs.is_empty() {
            return Ok(limits);
        }
        let mut local_peer = false;
        for registration in self.registrations.iter() {
            if registration.id == attachment || is_detached_in(self.detached, registration.id) {
                continue;
            }
            let Some(id) = cycle_of(registration) else {
                continue;
            };
            if id == cycle.id {
                continue;
            }
            let peer = self.cycle(id)?;
            let (topic, service) = {
                let theirs = peer.step_ref()?;
                (own.reads_topic_of(&theirs), own.services.is_peer_of(&theirs.services))
            };
            local_peer |= service;
            if !topic && !service {
                continue;
            }
            let Some(at) = self.peer_frontier(registration, peer)? else {
                continue;
            };
            let blocking = (format!("cycle '{}' of this node", peer.name()), None);
            let held = Limit { at: Some(at), blocking: Some(blocking) };
            if topic {
                limits.topic = limits.topic.min(held.clone());
            }
            if service {
                limits.service = limits.service.min(held);
            }
        }
        let own_node = self.node.transport.node_id();
        if !local_peer && own_node.is_some_and(|id| own.services.has_reporter(id)) {
            own.services.warn_outside_waitset(cycle.name());
        }
        Ok(limits)
    }

    /// The earliest step that `peer`, the cycle of `registration`, has not run, as this turn
    /// stands; `None` when it holds none. A cycle that has not seen the clock yet holds every
    /// step. A triggered peer's inputs are drained first, so a trigger that came since its
    /// last turn counts: its pending stamps of that turn would let this cycle run past it.
    fn peer_frontier(
        &self,
        registration: &Registration,
        peer: &Cycle,
    ) -> Result<Option<i64>, Error> {
        let due_later =
            self.due.iter().skip(self.due_index.saturating_add(1)).find_map(|due| match *due {
                Due::Replay(replay)
                    if replay.attachment == registration.id && replay.periods > 0 =>
                {
                    Some(replay.first_nanos)
                }
                _ => None,
            });
        let frontier = match registration.target {
            Target::Timer { ref timer, cycle: Some(ref owned), .. } => {
                if timer.awaiting_anchor() {
                    return Ok(Some(i64::MIN));
                }
                let sliced = owned.deferred.map(|window| window.first_nanos);
                let held = owned.upstream.hold.backlog_first();
                sliced.or(held).or(due_later).unwrap_or_else(|| timer.next_due().nanos())
            }
            Target::Triggered(ref state) => {
                if state.last_now.is_none() {
                    return Ok(Some(i64::MIN));
                }
                crate::cycle::drain_inputs(peer, crate::cycle::DrainClock::Simulated)?;
                let mut pending = Vec::new();
                peer.step_ref()?
                    .runtime
                    .pending_trigger_stamps(&mut pending)
                    .map_err(Error::from_code)?;
                return Ok(state.schedule.frontier(&pending, &state.limit));
            }
            _ => return Ok(None),
        };
        Ok(Some(due_later.map_or(frontier, |first| first.min(frontier))))
    }

    /// True when a cycle of this `WaitSet` holds another.
    fn has_local_edges(&self) -> bool {
        self.cycles.iter().any(|cycle| has_link(self.cycles, cycle))
    }

    /// Repeat this turn's timers while a cycle held by another cycle of this `WaitSet` can
    /// run a step that the other has since made possible.
    ///
    /// A repeat that runs a step, or moves the limit a triggered cycle reports (it follows
    /// the cycles that send what it reads, which a loop may route back to a held cycle),
    /// is followed by another. Without a step, the limits follow the rate cycles' frontiers
    /// and the pending triggers, which stay put, so they settle within one repeat per cycle
    /// of a chain; a loop of triggered cycles alone keeps its limits, as across nodes.
    pub(super) fn repeat_for_local_edges(&mut self, now: hypha_time::Time) -> Progression {
        if !self.has_local_edges() {
            return Progression::Continue;
        }
        let started = std::time::Instant::now();
        for _ in 0..MAX_REPEATS {
            if started.elapsed() >= super::replay::STEP_SLICE_BUDGET {
                break;
            }
            let before = (self.steps_run, self.limits_moved);
            let decision = self.fire_due(now, true);
            if decision != Progression::Continue {
                return decision;
            }
            if (self.steps_run, self.limits_moved) == before {
                break;
            }
        }
        Progression::Continue
    }
}
