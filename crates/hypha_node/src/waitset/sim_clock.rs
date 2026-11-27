//! The simulated-time half of a `WaitSet` run: the `/clock` and `/clock_progress` drains,
//! and the progress that the cycle's turn offers.

use hypha_runtime::replay_progress::{ProgressTurn, progress_turn};

use super::dispatch::{fail_with, is_detached_in};
use super::{BatchStep, BatchTurn, CallbackDispatcher, Progression, ReceiveFailure, Target};
use crate::cycle::DrainClock;
use crate::node_progress::{self, ClockRead};
use crate::status::Error;
use crate::subscriber::HyphaSubscriber;

impl CallbackDispatcher<'_> {
    /// Advance simulated time from `/clock`, then read the reports on `/clock_progress`,
    /// before evaluating timers or callbacks.
    ///
    /// Each `/clock` sample is recorded on the node still under the input's lock, so the
    /// recorded newest is never older than the clock. A sample lower than the newest one
    /// the node applied is not applied and ends the run with `ClockRewound`, whatever the
    /// `WaitSet` holds and in the lockstep or not: nothing rewinds in place (#130). A CLI
    /// tool follows the clock back.
    pub(super) fn drain_sim_clock(&mut self) -> Result<(), Progression> {
        let node = self.node;
        let refuses_rewind = !hypha_core::port::is_cli_node_name(&node.name);
        self.drain_topic(&node.sim_clock_subscriber, |sample| {
            let read = super::callbacks::read_sim_clock_sample(sample)?;
            if refuses_rewind
                && let Some(newest) = node_progress::newest_read(node)
                && read.nanos < newest.nanos
            {
                return Err(Error::from_status(crate::replay_restart::refuse_clock_rewind(
                    &node.name,
                    newest.nanos,
                    read.nanos,
                )));
            }
            super::callbacks::apply_sim_clock_read(read)?;
            node_progress::record_clock_read(node, read);
            Ok(())
        })?;
        self.drain_topic(&node.progress_subscriber, |sample| {
            node_progress::accept_report(node, sample).map_err(Error::from_status)
        })?;
        node_progress::note_progress_gaps(node);
        Ok(())
    }

    /// Apply the samples queued on one framework input, in one bounded batch. A sample
    /// that cannot be applied fails the run.
    pub(super) fn drain_topic<T>(
        &mut self,
        subscriber_mutex: &std::sync::Mutex<Option<Box<HyphaSubscriber>>>,
        mut apply: impl FnMut(&hypha_iceoryx2::Iox2Sample) -> Result<T, Error>,
    ) -> Result<Option<T>, Progression> {
        let Ok(mut guard) = subscriber_mutex.try_lock() else {
            return Ok(None);
        };
        let Some(subscriber) = guard.as_deref_mut() else {
            return Ok(None);
        };
        let mut turn = BatchTurn::new(self.batch_size);
        let mut last = None;
        loop {
            let step = match subscriber.receive_validated() {
                Ok(Some(sample)) => match apply(&sample) {
                    Ok(value) => {
                        last = Some(value);
                        BatchStep::Accepted
                    }
                    Err(status) => return Err(fail_with(&mut self.failure, 0, status)),
                },
                Err(ReceiveFailure::Refused(_)) => BatchStep::Refused,
                Ok(None) | Err(ReceiveFailure::Transport(_)) => BatchStep::Exhausted,
            };
            if !turn.record(step) {
                break;
            }
        }
        Ok(last)
    }

    /// Drain the inputs of every replay cycle of this `WaitSet`, after the turn's steps and
    /// other callbacks, which may have published into them; a triggered cycle drains on
    /// each of its turns. The progress offered next then answers no sample whose data still
    /// waits in a transport queue: each queue holds only what was sent since, and a loss
    /// shows as a history eviction, which the capture judges (replay-flow-control.md §4.2).
    pub(super) fn drain_replay_inputs(&mut self) -> Progression {
        let detached = self.detached;
        let replay =
            self.registrations.iter().filter_map(|registration| match registration.target {
                Target::Timer { ref timer, cycle: Some(ref owned), .. }
                    if timer.is_grid() && !is_detached_in(detached, registration.id) =>
                {
                    Some((registration.id, owned.cycle))
                }
                _ => None,
            });
        for (attachment, id) in replay {
            let drained = self
                .cycle(id)
                .and_then(|cycle| crate::cycle::drain_inputs(cycle, DrainClock::Simulated));
            if let Err(status) = drained {
                return fail_with(&mut self.failure, attachment, status);
            }
        }
        Progression::Continue
    }

    /// True when this `WaitSet` runs a replay cycle, whose steps the progress reports.
    pub(super) fn has_replay_timer(&self) -> bool {
        self.registrations.iter().any(|registration| {
            matches!(
                registration.target,
                Target::Timer { ref timer, cycle: Some(_), .. } if timer.is_grid()
            )
        })
    }

    /// True when this `WaitSet` has steps the progress reports: a replay cycle or a
    /// triggered cycle. Plain callbacks take no part in the replay.
    fn reports_progress(&self) -> bool {
        self.has_replay_timer()
            || self
                .registrations
                .iter()
                .any(|registration| matches!(registration.target, Target::Triggered(_)))
    }

    /// The node's newest `/clock` sample. Another `WaitSet` of the node may have drained it.
    pub(super) fn newest_read(&self) -> Option<ClockRead> {
        node_progress::newest_read(self.node)
    }

    /// Offer the `snapshot` of the newest `/clock` sample, taken before the clock was read
    /// as `now`, as this `WaitSet`'s progress.
    pub(super) fn publish_progress(&mut self, snapshot: Option<ClockRead>, now: hypha_time::Time) {
        match progress_turn(
            snapshot,
            now.nanos(),
            || self.reports_progress(),
            || self.progress_frontier(),
        ) {
            ProgressTurn::Hold => {}
            ProgressTurn::Withdraw => node_progress::withdraw_progress(self.node, self.waitset),
            ProgressTurn::Offer(read) => {
                node_progress::report_progress(self.node, self.waitset, read);
            }
        }
    }

    /// The earliest deadline that this `WaitSet` has not run. `None` when nothing holds one.
    fn progress_frontier(&self) -> Option<i64> {
        self.registrations
            .iter()
            .filter_map(|registration| match registration.target {
                Target::Timer { ref timer, cycle: Some(ref cycle), .. }
                    if timer.is_grid() && !timer.awaiting_anchor() =>
                {
                    let sliced = cycle.deferred.map(|window| window.first_nanos);
                    let held = cycle.upstream.hold.backlog_first();
                    Some(sliced.or(held).unwrap_or(timer.next_due().nanos()))
                }
                Target::Triggered(ref state) => {
                    state.schedule.frontier(&state.pending, &state.limit)
                }
                _ => None,
            })
            .min()
    }
}
