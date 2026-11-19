//! `hypha record --sync`: the recorder as a participant of a `/clock` replay.
//!
//! The recorder acknowledges a `/clock` sample only once it has written everything the
//! replay sent before that sample. It never waits for its own report.

use std::time::Instant;

use hypha_node::sdk::{BusNode, PublisherOptions, RawPublisher, RawSubscriber};

use crate::CLOCK_TOPIC;
use crate::replay_bus;
use crate::replay_sync::{self, PROGRESS_DATA_TYPE, PROGRESS_DEFINITION, PROGRESS_TOPIC, Progress};
use hypha_node::sdk::clock_barrier::{self, PROGRESS_MAX_PAYLOAD_BYTES, Participants};

/// Samples of [`CLOCK_TOPIC`] the recorder keeps while it drains them.
const CLOCK_DEPTH: usize = 8;
/// How often an unanswered recorder repeats its announcement to a player.
const HELLO_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// The recorder's side of the replay protocol, without the bus.
#[derive(Debug)]
pub struct SyncState {
    own: String,
    /// Nodes that must have reported before the first sample is reported.
    required: Vec<String>,
    participants: Participants,
    /// The newest `/clock` sample: its value and its host send time.
    newest: Option<(u64, u64)>,
    /// The send time of the sample the recorder last reported.
    reported_sent: Option<u64>,
}

impl SyncState {
    /// No sample and no participant. `own` is the recorder's node name; `required` are
    /// the nodes the recorder must have heard before it reports its first sample.
    #[must_use]
    pub fn new(own: String, required: Vec<String>, now: Instant) -> Self {
        Self {
            own,
            required,
            participants: Participants::new(now),
            newest: None,
            reported_sent: None,
        }
    }

    /// Keeps the sample with the latest send time. A sample whose clock is lower than
    /// the newest one is a backward seek: the others restart, so the recorder forgets them.
    pub fn on_clock(&mut self, nanos: u64, sent: u64, now: Instant) {
        let newer = self.newest.is_none_or(|(_, newest_sent)| sent >= newest_sent);
        if !newer {
            return;
        }
        if self.newest.is_some_and(|(newest, _)| nanos < newest) {
            self.participants = Participants::new(now);
        }
        self.newest = Some((nanos, sent));
    }

    /// Records another node's report. The recorder's own reports are not participants.
    pub fn on_report(&mut self, report: &Progress, now: Instant) {
        if report.node != self.own {
            self.participants.observe(
                &report.node,
                report.clock_nanos,
                report.clock_sent_nanos,
                now,
            );
        }
    }

    /// The report to send now, when the newest sample has not been reported and every
    /// participant has acknowledged it. Its clock is the sample's value, which the
    /// recorder can claim only because it has drained the followed topics first.
    #[must_use]
    pub fn due(&self) -> Option<Progress> {
        let (clock_nanos, sent) = self.newest?;
        if self.reported_sent.is_some_and(|reported| sent <= reported) {
            return None;
        }
        if self.reported_sent.is_none()
            && self.required.iter().any(|name| self.participants.get(name).is_none())
        {
            return None;
        }
        let required = self.participants.names();
        if !clock_barrier::unacknowledged(&self.participants, &required, sent, clock_nanos)
            .is_empty()
        {
            return None;
        }
        Some(Progress { node: self.own.clone(), clock_nanos, clock_sent_nanos: sent })
    }

    fn has_reported(&self) -> bool {
        self.reported_sent.is_some()
    }

    fn own(&self) -> &str {
        &self.own
    }

    /// Notes that the report for the sample sent at `sent` went out.
    pub fn reported(&mut self, sent: u64) {
        self.reported_sent = Some(sent);
    }
}

/// The recorder's bus endpoints for a replay: the `/clock` samples it reads, the reports
/// of the other participants, and its own report publisher.
pub struct SyncParticipant {
    clock: RawSubscriber,
    progress_in: RawSubscriber,
    progress_out: RawPublisher,
    state: SyncState,
    /// When the recorder last announced itself.
    last_hello: Instant,
}

impl SyncParticipant {
    /// Opens the endpoints as the recorder's own node. `required` are the nodes the first
    /// report waits for, as [`SyncState::new`] takes them.
    ///
    /// # Errors
    /// Returns a message when the node name, a subscription, or the report publisher
    /// cannot be created, or when `required` names the recorder itself.
    pub fn open(bus: &BusNode, required: Vec<String>) -> Result<Self, String> {
        let own = bus.name()?;
        if required.contains(&own) {
            return Err(format!(
                "--nodes names the recorder itself ('{own}'), which never reports to itself"
            ));
        }
        let clock = bus
            .subscribe_raw_with_schema(
                CLOCK_TOPIC,
                hypha_node::sdk::msgs::rosgraph_msgs::Clock::TYPE_NAME,
                hypha_node::sdk::msgs::rosgraph_msgs::Clock::SCHEMA_HASH,
                hypha_node::sdk::SubscriberOptions::default().keeping(CLOCK_DEPTH),
            )
            .map_err(|error| format!("cannot read {CLOCK_TOPIC}: {error}"))?;
        let progress_in = replay_bus::progress_subscriber(bus)
            .map_err(|error| format!("cannot read {PROGRESS_TOPIC}: {error}"))?;
        let progress_out = bus
            .publish_raw_configured(
                PROGRESS_TOPIC,
                PROGRESS_DATA_TYPE,
                Progress::SCHEMA_HASH,
                PROGRESS_DEFINITION,
                PublisherOptions::default().bounded(PROGRESS_MAX_PAYLOAD_BYTES),
            )
            .map_err(|error| format!("cannot publish {PROGRESS_TOPIC}: {error}"))?;
        let mut participant = Self {
            clock,
            progress_in,
            progress_out,
            state: SyncState::new(own, required, Instant::now()),
            last_hello: Instant::now(),
        };
        participant.announce()?;
        Ok(participant)
    }

    /// Makes a player aware of the recorder before it has answered a sample: a report that
    /// acknowledges nothing (send time 0). Repeats it until the first sample is answered,
    /// because `/clock_progress` keeps no history.
    /// # Errors
    /// Returns a message when the announcement cannot be published.
    pub fn keep_known(&mut self) -> Result<(), String> {
        if self.state.has_reported() || self.last_hello.elapsed() < HELLO_INTERVAL {
            return Ok(());
        }
        self.announce()
    }

    fn announce(&mut self) -> Result<(), String> {
        self.last_hello = Instant::now();
        let hello =
            Progress { node: self.state.own().to_string(), clock_nanos: 0, clock_sent_nanos: 0 };
        let payload = replay_sync::encode_progress(&hello)?;
        if self.progress_out.publish(&payload) {
            Ok(())
        } else {
            Err(format!("cannot publish {PROGRESS_TOPIC}"))
        }
    }

    /// Reads every queued `/clock` sample and progress report, then returns the report
    /// due now, if any.
    ///
    /// # Errors
    /// Returns a message when a input cannot be read or holds a malformed sample.
    pub fn poll(&mut self) -> Result<Option<Progress>, String> {
        let now = Instant::now();
        while let Some(received) = self
            .clock
            .try_receive()
            .map_err(|error| format!("cannot read {CLOCK_TOPIC}: {error}"))?
        {
            let bytes =
                received.bytes().ok_or_else(|| format!("a malformed sample on {CLOCK_TOPIC}"))?;
            let nanos = hypha_node::sdk::RosTime::decode_clock(bytes)
                .map(hypha_node::sdk::RosTime::as_nanos)
                .ok_or_else(|| format!("{CLOCK_TOPIC}: not one ROS time under a second"))?;
            let sent = received
                .identity()
                .ok_or_else(|| format!("a malformed sample on {CLOCK_TOPIC}"))?
                .transport_sent_nanos;
            self.state.on_clock(nanos, sent, Instant::now());
        }
        while let Some(received) = self
            .progress_in
            .try_receive()
            .map_err(|error| format!("cannot read {PROGRESS_TOPIC}: {error}"))?
        {
            let bytes = received
                .bytes()
                .ok_or_else(|| format!("a malformed sample on {PROGRESS_TOPIC}"))?;
            let report = replay_sync::decode_progress(bytes)
                .map_err(|error| format!("{PROGRESS_TOPIC}: {error}"))?;
            self.state.on_report(&report, now);
        }
        Ok(self.state.due())
    }

    /// Publishes `report` and records that its sample is answered.
    ///
    /// # Errors
    /// Returns a message when the report cannot be encoded or published.
    pub fn send(&mut self, report: &Progress) -> Result<(), String> {
        let payload = replay_sync::encode_progress(report)?;
        if !self.progress_out.publish(&payload) {
            return Err(format!("cannot publish {PROGRESS_TOPIC}"));
        }
        self.state.reported(report.clock_sent_nanos);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const OWN: &str = "hypha_record";

    fn report(node: &str, clock_nanos: u64, clock_sent_nanos: u64) -> Progress {
        Progress { node: node.to_string(), clock_nanos, clock_sent_nanos }
    }

    fn state() -> SyncState {
        SyncState::new(OWN.to_string(), Vec::new(), Instant::now())
    }

    #[test]
    fn alone_the_newest_sample_is_due_at_once() {
        let mut sync = state();
        sync.on_clock(100, 1_000, Instant::now());
        assert_eq!(sync.due(), Some(report(OWN, 100, 1_000)));
    }

    #[test]
    fn it_waits_for_another_participant_to_acknowledge_the_sample() {
        let mut sync = state();
        sync.on_report(&report("node_a", 50, 900), Instant::now());
        sync.on_clock(100, 1_000, Instant::now());
        assert_eq!(sync.due(), None, "node_a has not reached the sample");
        sync.on_report(&report("node_a", 100, 1_000), Instant::now());
        assert_eq!(sync.due(), Some(report(OWN, 100, 1_000)));
    }

    #[test]
    fn the_first_sample_waits_for_each_named_node_to_report() {
        let mut sync = SyncState::new(OWN.to_string(), vec!["node_a".to_string()], Instant::now());
        sync.on_clock(100, 1_000, Instant::now());
        assert_eq!(sync.due(), None, "node_a has not been heard");
        sync.on_report(&report("node_b", 100, 1_000), Instant::now());
        assert_eq!(sync.due(), None, "only the named node releases the first sample");
        sync.on_report(&report("node_a", 100, 1_000), Instant::now());
        assert_eq!(sync.due(), Some(report(OWN, 100, 1_000)));
    }

    #[test]
    fn its_own_reports_are_not_participants() {
        let mut sync = state();
        sync.on_report(&report(OWN, 0, 0), Instant::now());
        sync.on_clock(100, 1_000, Instant::now());
        assert_eq!(sync.due(), Some(report(OWN, 100, 1_000)));
    }

    #[test]
    fn a_partial_report_does_not_release_the_sample() {
        let mut sync = state();
        sync.on_report(&report("node_a", 100, 1_000), Instant::now());
        sync.on_clock(200, 2_000, Instant::now());
        sync.on_report(&report("node_a", 150, 2_000), Instant::now());
        assert_eq!(sync.due(), None, "node_a holds deadlines below 200");
        sync.on_report(&report("node_a", 200, 2_000), Instant::now());
        assert_eq!(sync.due(), Some(report(OWN, 200, 2_000)));
    }

    #[test]
    fn a_newer_sample_supersedes_the_older_one() {
        let mut sync = state();
        sync.on_clock(100, 1_000, Instant::now());
        sync.on_clock(200, 2_000, Instant::now());
        sync.on_clock(50, 500, Instant::now());
        assert_eq!(sync.due(), Some(report(OWN, 200, 2_000)));
    }

    #[test]
    fn a_rewind_supersedes_the_samples_before_it() {
        let mut sync = state();
        sync.on_clock(900, 1_000, Instant::now());
        sync.on_clock(100, 2_000, Instant::now());
        assert_eq!(sync.due(), Some(report(OWN, 100, 2_000)));
    }

    /// The others restart for a backward seek and rejoin later: the recorder answers the
    /// sample that goes back at once, so the player sees it stay.
    #[test]
    fn a_clock_that_goes_back_forgets_the_participants_that_restart() {
        let mut sync = state();
        let now = Instant::now();
        sync.on_clock(900, 1_000, now);
        sync.on_report(&report("a", 900, 1_000), now);
        sync.reported(sync.due().expect("due").clock_sent_nanos);
        sync.on_clock(100, 2_000, now);
        assert_eq!(sync.due(), Some(report(OWN, 100, 2_000)));
    }

    #[test]
    fn nothing_is_due_twice_for_one_sample() {
        let mut sync = state();
        sync.on_clock(100, 1_000, Instant::now());
        let first = sync.due().expect("due");
        sync.reported(first.clock_sent_nanos);
        assert_eq!(sync.due(), None);
        sync.on_clock(100, 1_000, Instant::now());
        assert_eq!(sync.due(), None, "the same sample again is not new");
        sync.on_clock(100, 3_000, Instant::now());
        assert_eq!(sync.due(), Some(report(OWN, 100, 3_000)), "a resend is answered again");
    }
}
