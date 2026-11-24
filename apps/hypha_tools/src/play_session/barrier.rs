//! The `/clock` barrier: participant reports, the clock sample, and the wait for acknowledgements.

use super::{CLOCK_RESEND_INTERVAL, CLOCK_TOPIC, CONTROL_POLL_INTERVAL, Flow, Session, reports};
use crate::replay_sync;
use crate::replay_sync::PROGRESS_TOPIC;
use hypha_node::sdk::clock_barrier::{first_silent, unacknowledged};
use std::time::{Duration, Instant};

impl Session<'_> {
    /// Runs the executor for `wait`, then reads the participants' reports.
    pub(super) fn serve(&mut self, wait: Duration) -> Result<(), String> {
        if let Some(error) = self.waitset.run_for(wait).err() {
            return Err(format!("the replay executor failed: {error}"));
        }
        self.drain_progress()
    }

    /// Reads every report queued on [`PROGRESS_TOPIC`] into the participants.
    pub(super) fn drain_progress(&mut self) -> Result<(), String> {
        let Some(progress) = self.progress.as_ref() else {
            return Ok(());
        };
        let taken = reports::take(progress);
        if taken.malformed {
            return Err(format!("a malformed sample on {PROGRESS_TOPIC}"));
        }
        if taken.dropped > 0 {
            eprintln!(
                "hypha play: {} report(s) on {PROGRESS_TOPIC} dropped: the player did not read them in time",
                taken.dropped
            );
        }
        let now = Instant::now();
        for bytes in &taken.payloads {
            let report = replay_sync::decode_progress(bytes)
                .map_err(|error| format!("{PROGRESS_TOPIC}: {error}"))?;
            if self.participants.observe(
                &report.node,
                report.clock_nanos,
                report.clock_sent_nanos,
                now,
            ) {
                println!("hypha play: '{}' takes part in the replay", report.node);
            }
        }
        Ok(())
    }

    /// Publishes the `/clock` sample for `log_time`, stamped with the same value.
    pub(super) fn publish_clock_frame(&mut self, log_time: u64) -> Result<(), String> {
        let payload = replay_sync::clock_payload(log_time)?;
        let Some(clock) = self.clock.as_mut() else {
            return Err("cannot publish UTC replay clock: no clock publisher".to_string());
        };
        if clock.publish_at_framework_nanos(&payload, log_time) {
            Ok(())
        } else {
            Err(format!("cannot publish {CLOCK_TOPIC}"))
        }
    }

    /// Sends a new `/clock` sample and returns the floor for its acknowledgements: the
    /// host monotonic time just before the send.
    pub(super) fn send_clock(&mut self, log_time: u64) -> Result<u64, String> {
        let floor = u64::try_from(hypha_node::sdk::clock::steady_now_nanos())
            .map_err(|_| "the host monotonic clock reads a negative time".to_string())?;
        self.publish_clock_frame(log_time)?;
        Ok(floor)
    }

    /// Waits until every participant has answered a `/clock` sample for `log_time` sent at or
    /// after `floor`, with its clock at or beyond `log_time`. While it waits the sample is sent
    /// again, and a participant that stays quiet for the timeout stops the replay.
    pub(super) fn await_acks(
        &mut self,
        log_time: u64,
        floor: u64,
        sent_at: Instant,
    ) -> Result<Flow, String> {
        let required = self.participants.names();
        let started = Instant::now();
        let mut last_send = started;
        // Each participant's first answer, as this loop saw it, for the rate report.
        let mut first_acks: Vec<(String, Instant)> = Vec::new();
        loop {
            if self.stopping() {
                return Ok(Flow::Halt);
            }
            let now = Instant::now();
            // Only the nodes still awaited can be silent: one that answered has done its part.
            let pending = unacknowledged(&self.participants, &required, floor, log_time);
            for node in &required {
                if !pending.contains(node) && !first_acks.iter().any(|(name, _)| name == node) {
                    first_acks.push((node.clone(), now));
                }
            }
            if pending.is_empty() {
                self.report.record_barrier(sent_at, &first_acks);
                return Ok(Flow::Go);
            }
            let timeout = self.config.sync.timeout;
            if let Some(silent) = first_silent(&self.participants, &pending, started, now, timeout)
            {
                let last = silent.last_clock_nanos.map_or_else(
                    || "it never reported".to_string(),
                    |nanos| format!("its last clock was {nanos} ns"),
                );
                return Err(format!(
                    "hypha play: replay participant '{}' stopped reporting: {last}; no progress for {timeout:?}",
                    silent.node
                ));
            }
            if now.saturating_duration_since(last_send) >= CLOCK_RESEND_INTERVAL {
                self.publish_clock_frame(log_time)?;
                last_send = now;
            }
            // A report wakes the wait; the slice bounds it for keys and the resend.
            self.idle(CONTROL_POLL_INTERVAL)?;
        }
    }

    /// Sends `/clock` for `log_time` and, in the lockstep, waits for every participant to
    /// answer it. An open-loop playback sends it and goes on.
    pub(super) fn barrier(&mut self, log_time: u64) -> Result<Flow, String> {
        if !self.in_lockstep() {
            self.publish_clock_frame(log_time)?;
            return Ok(Flow::Go);
        }
        let sent_at = Instant::now();
        let floor = self.send_clock(log_time)?;
        self.await_acks(log_time, floor, sent_at)
    }

    /// Counts the data up to `boundary` as published. Called once the chunk is acknowledged.
    pub(super) fn account_data(&mut self, boundary: u64) {
        self.report.add_data(boundary.saturating_sub(self.last_boundary));
        self.last_boundary = boundary;
    }
}
