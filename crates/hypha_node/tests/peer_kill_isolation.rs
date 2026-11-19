//! A killed peer costs the surviving nodes nothing (FSR-27, freedom from interference).
//!
//! Each test kills a real child process with SIGKILL while it uses a topic that a
//! surviving publisher and a surviving subscriber share, and then checks the survivors
//! through the SDK: every frame of the surviving publisher arrives once and in order,
//! no publish is refused, and the subscriber rejects no frame.
//!
//! The peer is `tests/support/kill_peer.rs`. The tests need writable iceoryx2
//! shared-memory paths (`/dev/shm`, `/tmp/iceoryx2`), outside a sandbox.

#![allow(clippy::expect_used, reason = "a failed setup or receive is a test failure")]

use std::io::{BufRead as _, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use hypha_node::sdk::{BusNode, RawPublisher, RawSubscriber};
use hypha_testkit::unique_name;

const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const SURVIVOR_TAG: u8 = b's';
const DEADLINE: Duration = Duration::from_secs(10);
/// More frames than a publisher's segment has slots (547 for the `default` policy), so
/// a slot that a killed reader kept for ever would show as a refused publish.
const FRAMES_AFTER_THE_KILL: u64 = 2000;

/// A running peer and the pipe it reports on.
struct Peer {
    child: Child,
    lines: BufReader<ChildStdout>,
}

impl Peer {
    /// Starts the peer in `mode` and returns when its endpoint exists.
    fn start(mode: &str, node: &str, topic: &str, tag: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hypha_node_kill_peer"))
            .args([mode, node, topic, tag])
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn the peer");
        let stdout = child.stdout.take().expect("the peer's stdout is piped");
        let mut peer = Self { child, lines: BufReader::new(stdout) };
        peer.expect_line("ready");
        peer
    }

    fn expect_line(&mut self, expected: &str) {
        let mut line = String::new();
        self.lines.read_line(&mut line).expect("read a line from the peer");
        assert_eq!(line.trim(), expected, "the peer ended or reported something else");
    }

    /// SIGKILL: no destructor of the peer runs, as in a crash.
    fn kill(mut self) {
        self.child.kill().expect("kill the peer");
        self.child.wait().expect("reap the peer");
    }
}

/// The survivors of a test and what the subscriber received so far.
struct Survivors {
    publisher: RawPublisher,
    subscriber: RawSubscriber,
    next_to_send: u64,
    /// Counters of the surviving publisher's frames, in arrival order.
    survivor_frames: Vec<u64>,
    /// Tag and counter of every frame of a peer, in arrival order.
    peer_frames: Vec<(u8, u64)>,
}

impl Survivors {
    fn new(bus: &BusNode, topic: &str) -> Self {
        let publisher = bus.publish_raw(topic, "test/Bytes", MD5, "").expect("publisher");
        let subscriber = bus.subscribe_raw(topic, 0).expect("subscriber");
        Self {
            publisher,
            subscriber,
            next_to_send: 0,
            survivor_frames: Vec::new(),
            peer_frames: Vec::new(),
        }
    }

    /// Takes every queued sample and files it by its tag.
    fn drain(&mut self) {
        while let Some(sample) = self.subscriber.try_receive().expect("receive") {
            let bytes = sample.bytes().expect("an accepted sample has a payload");
            let (tag, counter_bytes) = bytes.split_first().expect("a frame starts with its tag");
            let counter =
                u64::from_le_bytes(counter_bytes.try_into().expect("an eight byte counter"));
            if *tag == SURVIVOR_TAG {
                self.survivor_frames.push(counter);
            } else {
                self.peer_frames.push((*tag, counter));
            }
        }
    }

    /// Publishes `count` frames of the survivor, and reads the queue after each one.
    fn publish_and_read(&mut self, count: u64, pause: Duration) {
        for _ in 0..count {
            let mut frame = [SURVIVOR_TAG; 9];
            for (target, source) in frame.iter_mut().skip(1).zip(self.next_to_send.to_le_bytes()) {
                *target = source;
            }
            let sent = self.publisher.publish(&frame);
            assert!(sent, "publish {} of the surviving publisher was refused", self.next_to_send);
            self.next_to_send = self.next_to_send.wrapping_add(1);
            self.drain();
            if !pause.is_zero() {
                std::thread::sleep(pause);
            }
        }
    }

    /// Reads until `enough` says so, or fails after the deadline with `waiting_for`.
    fn read_until(&mut self, waiting_for: &str, enough: impl Fn(&Self) -> bool) {
        let started = Instant::now();
        loop {
            self.drain();
            if enough(self) {
                return;
            }
            assert!(started.elapsed() < DEADLINE, "no {waiting_for} within {DEADLINE:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Every frame of the survivor arrived once, in order, and nothing was rejected.
    fn assert_intact(&mut self) {
        self.drain();
        let expected: Vec<u64> = (0..self.next_to_send).collect();
        assert_eq!(self.survivor_frames, expected, "the surviving publisher's frames");
        let metrics = self.subscriber.metrics().expect("metrics");
        let rejections: Vec<&(String, u64)> = metrics
            .iter()
            .filter(|(name, value)| name.starts_with("integrity.") && *value != 0)
            .collect();
        assert!(rejections.is_empty(), "the subscriber rejected frames: {rejections:?}");
    }
}

fn topic_of(label: &str) -> String {
    format!("/hypha_node/peer_kill/{}", unique_name(label))
}

#[test]
fn a_killed_publisher_costs_the_surviving_publisher_no_frame() {
    let bus = BusNode::new(&unique_name("kill_publisher_survivor")).expect("node");
    let topic = topic_of("publisher");
    let mut survivors = Survivors::new(&bus, &topic);

    let peer = Peer::start("flood", &unique_name("kill_publisher_peer"), &topic, "a");
    survivors.publish_and_read(100, Duration::from_millis(1));
    survivors.read_until("frame of the peer", |seen| !seen.peer_frames.is_empty());

    peer.kill();
    survivors.publish_and_read(200, Duration::from_millis(1));

    survivors.assert_intact();
}

#[test]
fn a_killed_reader_does_not_stop_its_publisher() {
    let bus = BusNode::new(&unique_name("kill_reader_survivor")).expect("node");
    let topic = topic_of("reader");
    let mut survivors = Survivors::new(&bus, &topic);

    let peer = Peer::start("read", &unique_name("kill_reader_peer"), &topic, "-");
    survivors.publish_and_read(100, Duration::from_millis(1));

    peer.kill();
    survivors.publish_and_read(FRAMES_AFTER_THE_KILL, Duration::ZERO);

    survivors.assert_intact();
    assert_eq!(survivors.subscriber.missing_sequences().expect("count"), 0);
}

#[test]
fn a_reader_killed_while_it_holds_samples_does_not_exhaust_the_pool() {
    let bus = BusNode::new(&unique_name("kill_holder_survivor")).expect("node");
    let topic = topic_of("holder");
    let mut survivors = Survivors::new(&bus, &topic);

    let mut peer = Peer::start("hold", &unique_name("kill_holder_peer"), &topic, "-");
    survivors.publish_and_read(8, Duration::ZERO);
    peer.expect_line("holding");

    peer.kill();
    survivors.publish_and_read(FRAMES_AFTER_THE_KILL, Duration::ZERO);

    survivors.assert_intact();
    assert_eq!(survivors.subscriber.missing_sequences().expect("count"), 0);
}

#[test]
fn a_restarted_publisher_is_accepted_as_a_new_publisher() {
    let bus = BusNode::new(&unique_name("restart_publisher_survivor")).expect("node");
    let topic = topic_of("restart");
    let mut survivors = Survivors::new(&bus, &topic);
    let peer_node = unique_name("restart_publisher_peer");

    let first_run = Peer::start("flood", &peer_node, &topic, "a");
    survivors.read_until("frame of the first run", |seen| {
        seen.peer_frames.iter().filter(|(tag, _)| *tag == b'a').count() >= 50
    });
    first_run.kill();

    // The second run has the first one's node name, and its counter and its frame
    // sequence both start again at zero.
    let second_run = Peer::start("flood", &peer_node, &topic, "b");
    survivors.read_until("frame of the second run", |seen| {
        seen.peer_frames.iter().filter(|(tag, _)| *tag == b'b').count() >= 50
    });
    second_run.kill();

    let second: Vec<u64> = survivors
        .peer_frames
        .iter()
        .filter(|(tag, _)| *tag == b'b')
        .map(|(_, counter)| *counter)
        .take(50)
        .collect();
    let expected: Vec<u64> = (0..50).collect();
    assert_eq!(second, expected, "the second run is read from its first frame, in order");
    survivors.assert_intact();
}
