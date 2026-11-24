//! The start of a pass: the start barrier that waits for the participants, once, and the first `/clock`.

use super::{CLOCK_RESEND_INTERVAL, CONTROL_POLL_INTERVAL, Flow, Session};
use crate::replay_bus;
use crate::replay_sync::ChunkPlan;
use hypha_node::sdk::clock_barrier::{NAMED_START_TIMEOUT, StartState, start_state};
use std::time::Instant;

impl Session<'_> {
    /// The start barrier of the replay: waits until the participants are known, once.
    pub(super) fn discover(&mut self, log_time: u64) -> Result<Flow, String> {
        let started = Instant::now();
        let mut last_send = started;
        loop {
            if self.stopping() {
                return Ok(Flow::Halt);
            }
            let now = Instant::now();
            let named = &self.config.sync.nodes;
            // A publisher that has connected but not reported yet is a participant still to hear from.
            let publishers = self.progress_publisher_count()?;
            let state = start_state(
                &self.participants,
                named,
                started,
                now,
                self.config.sync.settle,
                publishers,
            );
            match state {
                StartState::Ready => return Ok(Flow::Go),
                StartState::NoneSeen => {
                    eprintln!(
                        "hypha play: no node reported progress within {:?}; pacing by --rate alone",
                        self.config.sync.settle
                    );
                    return Ok(Flow::Go);
                }
                StartState::Missing(missing) if self.rejoining => {
                    return Err(format!(
                        "hypha play: {} did not rejoin within {NAMED_START_TIMEOUT:?} after /clock went back; a participant must restart itself (hypha_node::sdk::runNode does)",
                        missing.join(", ")
                    ));
                }
                StartState::Missing(missing) => {
                    return Err(format!(
                        "hypha play: --nodes: {} reported no progress within {NAMED_START_TIMEOUT:?}",
                        missing.join(", ")
                    ));
                }
                StartState::Waiting => {}
            }
            if now.saturating_duration_since(last_send) >= CLOCK_RESEND_INTERVAL {
                self.publish_clock_frame(log_time)?;
                last_send = now;
            }
            self.idle(CONTROL_POLL_INTERVAL)?;
        }
    }

    /// Starts a pass: anchors it on its first log time, and holds the data until every
    /// participant has answered that `/clock` sample.
    pub(super) fn start_pass(&mut self, first_log_time: u64) -> Result<Flow, String> {
        let start = self.config.utc_range.start().unwrap_or(first_log_time);
        self.pass_start_log = start;
        self.last_boundary = start;
        self.plan = Some(ChunkPlan::new(start, self.config.sync.chunk_nanos)?);
        if self.clock.is_none() {
            self.schedule.reanchor(start, Instant::now());
            return Ok(Flow::Go);
        }
        // Open loop: the pass starts with its first /clock, and no node is waited for.
        if !self.in_lockstep() {
            self.publish_clock_frame(start)?;
            self.replay_started.get_or_insert_with(Instant::now);
            self.schedule.reanchor(start, Instant::now());
            return Ok(Flow::Go);
        }
        if self.restart_participants(start)? == Flow::Halt {
            return Ok(Flow::Halt);
        }
        let floor = self.send_clock(start)?;
        if !self.discovered {
            if self.discover(start)? == Flow::Halt {
                return Ok(Flow::Halt);
            }
            // Checked again: a second player that started at the same time has published by now.
            replay_bus::ensure_sole_clock_publisher(self.bus)?;
            self.discovered = true;
            if std::mem::take(&mut self.rejoining) {
                println!("hypha play: every participant rejoined");
            }
        }
        // The start barrier's own wait is not charged to the rate: the replay's wall time
        // starts once the participants are known, so the wait is taken from here.
        if self.await_acks(start, floor, Instant::now())? == Flow::Halt {
            return Ok(Flow::Halt);
        }
        self.replay_started.get_or_insert_with(Instant::now);
        self.schedule.reanchor(start, Instant::now());
        Ok(Flow::Go)
    }
}
