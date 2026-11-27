//! Helpers of the replay tests: a `/clock` participant and the ROS clock decoder.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use hypha_node::sdk::clock_barrier::PROGRESS_MAX_PAYLOAD_BYTES;
use hypha_node::sdk::{BusNode, PublisherOptions, RawPublisher, RawSubscriber};
use hypha_tools::replay_sync::{
    PROGRESS_DATA_TYPE, PROGRESS_DEFINITION, PROGRESS_TOPIC, Progress, encode_progress,
};

/// A child process killed when the test ends, passed or not.
#[allow(dead_code, reason = "not every test binary that includes this module spawns a tool")]
pub(crate) struct Killed(pub(crate) std::process::Child);

impl std::ops::Deref for Killed {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Killed {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Killed {
    fn drop(&mut self) {
        let exited = matches!(self.0.try_wait(), Ok(Some(_)));
        if !exited {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// One `/clock` sample as a participant reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClockSample {
    /// The UTC time the sample carries, in nanoseconds since the epoch.
    pub(crate) nanos: u64,
    /// The host monotonic time at which the player sent the sample.
    pub(crate) sent: u64,
}

/// A node that takes part in a replay: it reads `/clock` and reports its progress.
pub(crate) struct Responder<'bus> {
    bus: &'bus BusNode,
    name: String,
    clock: RawSubscriber,
    /// `None` while a restart for a backward seek has the node off the bus.
    progress: Option<RawPublisher>,
    /// The newest clock value received.
    newest: Option<u64>,
    /// The send time of the sample that went back, while the node is off the bus.
    rewound_at: Option<u64>,
    /// How long the node takes to leave the bus after the clock went back.
    pub(crate) leave_delay: std::time::Duration,
    /// When the clock went back, while the node has not left yet.
    leaving_since: Option<std::time::Instant>,
    /// The host monotonic times at which the node left the bus.
    pub(crate) left_at: Vec<u64>,
}

/// The progress publisher of a participant.
fn progress_publisher(bus: &BusNode) -> RawPublisher {
    let md5 = hypha_node::sdk::md5::md5_from_definition(PROGRESS_DATA_TYPE, PROGRESS_DEFINITION)
        .expect("the progress definition hashes");
    bus.publish_raw_configured(
        PROGRESS_TOPIC,
        PROGRESS_DATA_TYPE,
        &md5,
        PROGRESS_DEFINITION,
        PublisherOptions::default().bounded(PROGRESS_MAX_PAYLOAD_BYTES),
    )
    .expect("a progress publisher")
}

impl<'bus> Responder<'bus> {
    /// Subscribes to `/clock` and opens the progress publisher, as `name`.
    pub(crate) fn new(bus: &'bus BusNode, name: &str) -> Self {
        let clock = bus
            .subscribe_raw_with_schema(
                "/clock",
                hypha_node::sdk::msgs::rosgraph_msgs::Clock::TYPE_NAME,
                hypha_node::sdk::msgs::rosgraph_msgs::Clock::SCHEMA_HASH,
                hypha_node::sdk::SubscriberOptions::default().keeping(8),
            )
            .expect("a clock subscriber");
        Self {
            bus,
            name: name.to_string(),
            clock,
            progress: Some(progress_publisher(bus)),
            newest: None,
            rewound_at: None,
            leave_delay: std::time::Duration::ZERO,
            leaving_since: None,
            left_at: Vec::new(),
        }
    }

    /// Leaves the bus once [`Self::leave_delay`] has passed since the clock went back.
    fn leave_when_due(&mut self) {
        if self.leaving_since.is_some_and(|since| since.elapsed() >= self.leave_delay) {
            self.leaving_since = None;
            self.progress = None;
            let now = u64::try_from(hypha_node::sdk::clock::steady_now_nanos())
                .expect("a monotonic time");
            self.left_at.push(now);
        }
    }

    /// The `/clock` samples received since the last call, oldest first. A value that goes
    /// back takes the node off the bus, as a restart for a backward seek does; the next
    /// sample sent after it brings the node back.
    pub(crate) fn receive(&mut self) -> Vec<ClockSample> {
        let mut samples = Vec::new();
        self.leave_when_due();
        while let Some(sample) = self.clock.try_receive().expect("a clock receive") {
            let nanos = decode_clock(sample.bytes().expect("a clock payload"));
            let sent = sample.identity().expect("a clock identity").transport_sent_nanos;
            if self.newest.is_some_and(|newest| nanos < newest) {
                self.leaving_since = Some(std::time::Instant::now());
                self.leave_when_due();
                self.rewound_at = Some(sent);
            } else if self.progress.is_none()
                && self.rewound_at.is_some_and(|rewound| sent > rewound)
            {
                self.progress = Some(progress_publisher(self.bus));
                self.rewound_at = None;
            }
            self.newest = Some(nanos);
            samples.push(ClockSample { nanos, sent });
        }
        samples
    }

    /// Reports that this node has run `sample`, as the runtime reports each sample. A node
    /// off the bus for a restart reports nothing.
    pub(crate) fn answer(&mut self, sample: ClockSample) {
        let Some(progress) = self.progress.as_mut().filter(|_| self.rewound_at.is_none()) else {
            return;
        };
        let report = Progress {
            node: self.name.clone(),
            clock_nanos: sample.nanos,
            clock_sent_nanos: sample.sent,
        };
        let payload = encode_progress(&report).expect("a progress report");
        assert!(progress.publish(&payload), "the progress report was sent");
    }
}

/// Decodes a ROS `time` payload into Unix-epoch nanoseconds.
pub(crate) fn decode_clock(payload: &[u8]) -> u64 {
    let seconds =
        u32::from_ne_bytes(payload.get(..4).expect("seconds").try_into().expect("four bytes"));
    let nanos =
        u32::from_ne_bytes(payload.get(4..8).expect("nanos").try_into().expect("four bytes"));
    u64::from(seconds)
        .checked_mul(1_000_000_000)
        .and_then(|time| time.checked_add(u64::from(nanos)))
        .expect("UTC clock value")
}
