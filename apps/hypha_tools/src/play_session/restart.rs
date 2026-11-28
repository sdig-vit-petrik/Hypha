//! A pass that goes back in log time restarts the participants: no node rewinds in place.
//!
//! A restarted process joins only on a `/clock` sent after the one that rewound it, so
//! it never meets a process or a sample of the old timeline.

use super::{CONTROL_POLL_INTERVAL, Flow, Session, create_channel_publishers};
use hypha_node::sdk::clock_barrier::{NAMED_START_TIMEOUT, Participants, unacknowledged};
use std::time::Instant;

impl Session<'_> {
    /// Restarts the participants for a pass that starts at `start`, before its start
    /// barrier. Nothing to do before the first pass or without participants.
    pub(super) fn restart_participants(&mut self, start: u64) -> Result<Flow, String> {
        let names = self.participants.names();
        if !self.discovered || names.is_empty() {
            return Ok(Flow::Go);
        }
        println!("hypha play: /clock goes back; restarting {}", names.join(", "));
        let floor = self.send_clock(start)?;
        let started = Instant::now();
        loop {
            if self.stopping() {
                return Ok(Flow::Halt);
            }
            let publishers = self.progress_publisher_count()?;
            let staying = names
                .len()
                .saturating_sub(unacknowledged(&self.participants, &names, floor, start).len());
            let left = publishers.saturating_sub(staying);
            if left == 0 {
                break;
            }
            if started.elapsed() >= NAMED_START_TIMEOUT {
                return Err(format!(
                    "{left} participant(s) still on /clock_progress {NAMED_START_TIMEOUT:?} after /clock went back; a backward seek restarts every participant (hypha_node::sdk::runNode does)"
                ));
            }
            self.idle(CONTROL_POLL_INTERVAL)?;
        }
        // A latched sample of the old pass would reach a restarted subscriber.
        self.publishers.clear();
        self.publishers =
            create_channel_publishers(self.bus, self.player, &self.config)?.publishers;
        self.limits.clear();
        self.participants = Participants::new(Instant::now());
        self.config.sync.nodes = names;
        self.discovered = false;
        self.rejoining = true;
        Ok(Flow::Go)
    }
}
