//! The request and response edges between cycles of one `WaitSet` in a replay
//! (`docs/design/native-services.md` §4).
//!
//! A client and its server may be cycles of one node, which reports one progress for both,
//! so the node-wide upstream hold cannot order them. Within a turn each cycle runs all its
//! due steps before the next cycle runs, so without a hold of its own a server would run
//! a deadline before its client ran the step that called it. The edge between them holds
//! as an upstream edge does: a step at `t` runs once the peer cycle has run every step
//! before `t`. The turn repeats its timers while that lets a held step run.

use hypha_runtime::upstream_hold::Limit;

use super::dispatch::{CallbackDispatcher, Due, is_detached_in};
use super::{AttachmentId, Progression, Registration, Target};
use crate::cycle::{Cycle, CycleId};
use crate::status::Error;

/// The most repeats of a turn's timers for the edges between its own cycles. Each repeat
/// runs at least one held step, and a turn runs at most this many replay steps per cycle.
const MAX_REPEATS: u64 = super::replay::MAX_STEPS_PER_TURN;

/// The cycle a registration steps, with whether it is triggered.
fn cycle_of(registration: &Registration) -> Option<CycleId> {
    match registration.target {
        Target::Timer { ref timer, cycle: Some(ref owned), .. } if timer.is_grid() => {
            Some(owned.cycle)
        }
        Target::Triggered(ref state) => Some(state.cycle),
        _ => None,
    }
}

/// Whether `registration` steps a replay or triggered cycle with a server or a client.
pub(super) fn has_services(cycles: &[Cycle], registration: &Registration) -> bool {
    cycle_of(registration)
        .and_then(|id| cycles.iter().find(|cycle| cycle.id == id))
        .and_then(|cycle| cycle.step_ref().ok().map(|step| !step.services.is_empty()))
        .unwrap_or(false)
}

impl CallbackDispatcher<'_> {
    /// The limit that the service peers of `cycle` among the other cycles of this
    /// `WaitSet` give its next steps: each peer's earliest step not run yet, as a topic
    /// limit gives it (a deadline up to it may run).
    pub(super) fn local_service_limit(
        &self,
        attachment: AttachmentId,
        cycle: &Cycle,
    ) -> Result<Limit, Error> {
        let mut limit = Limit { at: None, blocking: None };
        let own = cycle.step_ref()?;
        if own.services.is_empty() {
            return Ok(limit);
        }
        for registration in self.registrations.iter() {
            if registration.id == attachment || is_detached_in(self.detached, registration.id) {
                continue;
            }
            let Some(id) = cycle_of(registration) else {
                continue;
            };
            let peer = self.cycle(id)?;
            if !own.services.is_peer_of(&peer.step_ref()?.services) {
                continue;
            }
            let Some(at) = self.peer_frontier(registration) else {
                continue;
            };
            let blocking = (format!("cycle '{}' of this node", peer.name()), None);
            limit = limit.min(Limit { at: Some(at), blocking: Some(blocking) });
        }
        Ok(limit)
    }

    /// The earliest step that the cycle of `registration` has not run, as this turn stands;
    /// `None` when it holds none. A cycle that has not seen the clock yet holds every step.
    fn peer_frontier(&self, registration: &Registration) -> Option<i64> {
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
                    return Some(i64::MIN);
                }
                let sliced = owned.deferred.map(|window| window.first_nanos);
                let held = owned.upstream.hold.backlog_first();
                sliced.or(held).or(due_later).unwrap_or_else(|| timer.next_due().nanos())
            }
            Target::Triggered(ref state) => {
                if state.last_now.is_none() {
                    return Some(i64::MIN);
                }
                return state.schedule.frontier(&state.pending, &state.limit);
            }
            _ => return None,
        };
        Some(due_later.map_or(frontier, |first| first.min(frontier)))
    }

    /// True when two cycles of this `WaitSet` are service peers.
    fn has_local_service_edges(&self) -> bool {
        let cycles: Vec<&Cycle> = self
            .registrations
            .iter()
            .filter_map(cycle_of)
            .filter_map(|id| self.cycle(id).ok())
            .collect();
        cycles.iter().enumerate().any(|(index, a)| {
            cycles.iter().skip(index.saturating_add(1)).any(|b| {
                match (a.step_ref(), b.step_ref()) {
                    (Ok(a), Ok(b)) => a.services.is_peer_of(&b.services),
                    _ => false,
                }
            })
        })
    }

    /// The earliest step not run of each replay cycle, in registration order.
    fn frontiers(&self) -> Vec<Option<i64>> {
        self.registrations.iter().map(|registration| self.peer_frontier(registration)).collect()
    }

    /// Repeat this turn's timers while a cycle held by a service peer of this `WaitSet` can
    /// run a step the peer has since made possible.
    pub(super) fn repeat_for_local_edges(&mut self, now: hypha_time::Time) -> Progression {
        if !self.has_local_service_edges() {
            return Progression::Continue;
        }
        for _ in 0..MAX_REPEATS {
            let before = self.frontiers();
            let decision = self.fire_due(now, true);
            if decision != Progression::Continue {
                return decision;
            }
            if self.frontiers() == before {
                break;
            }
        }
        Progression::Continue
    }
}
