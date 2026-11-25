//! A killed peer costs the surviving nodes nothing (FSR-27, freedom from interference).
//!
//! Each test kills a real child process with SIGKILL while it uses a topic that a
//! surviving publisher and a surviving subscriber share, and then checks the survivors
//! through the SDK: every frame of the surviving publisher arrives once and in order,
//! no publish is refused, and the subscriber rejects no frame.
//!
//! Two more tests kill a publisher that holds a frame it has not sent: one that keeps a
//! single loan, and one that stands for a cycle between the two sends of one step.
//!
//! The peer is `tests/support/kill_peer.rs`. The tests need writable iceoryx2
//! shared-memory paths (`/dev/shm`, `/tmp/iceoryx2`), outside a sandbox.

#![allow(clippy::expect_used, reason = "a failed setup or receive is a test failure")]

use std::io::{BufRead as _, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use hypha_node::cycle::{
    CycleSnapshots, CycleStepHandler, HYPHA_CYCLE_VERDICT_STALE, HyphaCycleBindingOptions,
    HyphaCycleBindingView, HyphaCycleEmission, HyphaCycleImage, HyphaCycleInputMode,
    HyphaCycleTime, HyphaInputSupervisionRequest,
};
use hypha_node::port_limits::HyphaSupervisionRanges;
use hypha_node::safe;
use hypha_node::sdk::{BusNode, RawPublisher, RawSubscriber};
use hypha_node::waitset::HyphaCallbackResult;
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

/// The frames the `loan` peer publishes before the loan it keeps; it publishes one more
/// after it (`kill_peer.rs`).
const FRAMES_BEFORE_THE_LOAN: u64 = 10;

#[test]
fn a_publisher_killed_with_an_unsent_loan_costs_the_survivors_nothing() {
    let bus = BusNode::new(&unique_name("kill_loan_survivor")).expect("node");
    let topic = topic_of("loan");
    let mut survivors = Survivors::new(&bus, &topic);
    let peer_node = unique_name("kill_loan_peer");

    let mut peer = Peer::start("loan", &peer_node, &topic, "a");
    survivors.publish_and_read(100, Duration::from_millis(1));
    peer.expect_line("loaned");
    survivors.read_until("frame the peer sent after its loan", |seen| {
        seen.peer_frames.len() > usize::try_from(FRAMES_BEFORE_THE_LOAN).expect("a few frames")
    });

    peer.kill();
    survivors.publish_and_read(FRAMES_AFTER_THE_KILL, Duration::ZERO);

    // The frame on loan was never sent: it does not arrive. It took no sequence number
    // either, so the frame sent after it leaves no gap.
    let sent: Vec<(u8, u64)> = (0..FRAMES_BEFORE_THE_LOAN)
        .chain([FRAMES_BEFORE_THE_LOAN.saturating_add(1)])
        .map(|counter| (b'a', counter))
        .collect();
    assert_eq!(survivors.peer_frames, sent, "the frames of the killed publisher");
    survivors.assert_intact();
    assert_eq!(survivors.subscriber.missing_sequences().expect("count"), 0);

    // The dead node's name, with its loan, is free for a new run.
    let second_run = Peer::start("flood", &peer_node, &topic, "b");
    survivors.read_until("frame of the second run", |seen| {
        seen.peer_frames.iter().filter(|(tag, _)| *tag == b'b').count() >= 50
    });
    second_run.kill();
    survivors.assert_intact();
}

/// The whole steps the `step` peer sends before the step it leaves half sent (`kill_peer.rs`).
const COMPLETE_STEPS: u64 = 3;
/// The silence limit of the consumer's two inputs.
const SILENCE_MILLIS: u64 = 200;
const CONSUMER_PERIOD: Duration = Duration::from_millis(10);

/// A step that reads nothing: the test reads the cycle's bindings between the runs.
struct EmptyStep;

impl CycleStepHandler for EmptyStep {
    fn step(
        &mut self,
        _image: *const HyphaCycleImage,
        _time: *const HyphaCycleTime,
        _emission: *mut HyphaCycleEmission,
    ) -> HyphaCallbackResult {
        HyphaCallbackResult::continue_run()
    }
}

/// A node whose parameter file gives the input ports `first` and `second` a silence limit.
fn consumer_node() -> std::sync::Arc<safe::HyphaNode> {
    let dir = std::env::temp_dir().join(unique_name("hypha_kill_step"));
    std::fs::create_dir_all(&dir).expect("parameter directory");
    let file = dir.join("params.toml");
    let limits = format!(
        "[ports.inputs.first]\nsilence_ms = {SILENCE_MILLIS}\n\
         [ports.inputs.second]\nsilence_ms = {SILENCE_MILLIS}\n"
    );
    std::fs::write(&file, limits).expect("parameter file");
    let node = safe::node_create(&unique_name("kill_step_consumer"), file.to_str(), None);
    std::fs::remove_dir_all(&dir).expect("remove the parameter directory");
    node.expect("node")
}

/// Binds a latest input of `cycle` to `topic` as the supervised port `port`.
#[expect(
    clippy::unnecessary_box_returns,
    reason = "the library hands out the box and takes it back"
)]
fn bind_supervised_input(
    node: &std::sync::Arc<safe::HyphaNode>,
    set: &safe::HyphaWaitSet,
    cycle: safe::CycleId,
    topic: &str,
    port: &str,
) -> Box<safe::HyphaInputSource> {
    let identity = safe::SubscriberIdentity {
        data_type: "test/Bytes",
        expected_md5: Some(MD5),
        definition: "",
    };
    let settings = safe::SubscriberSettings {
        depth: 8,
        no_publisher_warning_after_nanos: 0,
        request: hypha_core::port::QosRequest::Undeclared,
    };
    let subscriber = safe::subscribe_typed(node, topic, &identity, settings).expect("subscriber");
    let source = safe::input_source_create(set, Some(subscriber)).map_err(|(error, _)| error);
    let source = source.expect("input source");
    let latest = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 0,
        optional: 0,
    };
    let binding = safe::cycle_bind_input(set, cycle, &source, &latest).expect("input bound");
    // The function reads the node and the port name from its own arguments.
    let request = HyphaInputSupervisionRequest {
        node: std::ptr::null(),
        name: std::ptr::null(),
        name_len: 0,
        unsupervised: 0,
        check_stagnation: 0,
        takes_topic: 0,
        ranges: HyphaSupervisionRanges {
            silence_min_nanos: 1_000_000,
            silence_max_nanos: 3_600_000_000_000,
            stagnation_min_nanos: 1_000_000,
            stagnation_max_nanos: 3_600_000_000_000,
        },
    };
    let attached = safe::cycle_bind_input_supervision(set, cycle, node, &request, port, binding);
    assert!(attached.expect("supervision bound").is_some(), "the silence limit of '{port}'");
    source
}

/// The counters of every frame `reader` has, in arrival order.
fn counters(reader: &mut RawSubscriber) -> Vec<u64> {
    std::iter::from_fn(|| reader.try_receive().expect("receive"))
        .map(|sample| {
            let bytes = sample.bytes().expect("an accepted sample has a payload");
            let counter = bytes.get(1..).expect("a frame starts with its tag");
            u64::from_le_bytes(counter.try_into().expect("an eight byte counter"))
        })
        .collect()
}

#[test]
fn a_cycle_killed_between_two_sends_leaves_its_first_output_and_then_silence() {
    let topic = topic_of("step");
    let (first_topic, second_topic) = (format!("{topic}/first"), format!("{topic}/second"));
    let mut peer = Peer::start("step", &unique_name("kill_step_peer"), &topic, "a");

    let node = consumer_node();
    let lease = safe::node_default_waitset(&node).expect("waitset");
    let set = lease.waitset().expect("the lent waitset");
    let snapshots = std::sync::Arc::new(std::sync::OnceLock::new());
    let kept = std::sync::Arc::clone(&snapshots);
    let adopt = move |_, held: &std::sync::Arc<CycleSnapshots>| -> Box<dyn CycleStepHandler> {
        kept.get_or_init(|| std::sync::Arc::clone(held));
        Box::new(EmptyStep)
    };
    let period = u64::try_from(CONSUMER_PERIOD.as_nanos()).expect("a short period");
    let cycle = safe::cycle_create(set, "consumer", period, Some(adopt)).expect("cycle created");
    let first_source = bind_supervised_input(&node, set, cycle, &first_topic, "first");
    let second_source = bind_supervised_input(&node, set, cycle, &second_topic, "second");
    safe::cycle_commit(set, cycle).expect("cycle committed");
    // A plain reader of each output; the peer sends when both outputs have two readers.
    let bus = BusNode::new(&unique_name("kill_step_reader")).expect("node");
    let mut first_reader = bus.subscribe_raw(&first_topic, 0).expect("subscriber");
    let mut second_reader = bus.subscribe_raw(&second_topic, 0).expect("subscriber");

    peer.expect_line("between");
    peer.kill();

    // What the plain readers have: the first output of the last step, not the second.
    let sent_first: Vec<u64> = (0..=COMPLETE_STEPS).collect();
    let sent_second: Vec<u64> = (0..COMPLETE_STEPS).collect();
    assert_eq!(counters(&mut first_reader), sent_first, "the first output of every step");
    assert_eq!(counters(&mut second_reader), sent_second, "the second output of the whole steps");

    // What the cycle has: the same frames, and then both inputs stale, with no failure.
    let held = snapshots.get().expect("the cycle's snapshots");
    let binding = |index| safe::cycle_binding(held, index).expect("a bound input");
    let stale =
        |view: &HyphaCycleBindingView| view.health.capture.verdict == HYPHA_CYCLE_VERDICT_STALE;
    let started = Instant::now();
    loop {
        let report = safe::waitset_run(set, Some(CONSUMER_PERIOD)).expect("run");
        assert_eq!(report.status, 0, "a dead producer fails no step of its consumer");
        let (first, second) = (binding(0), binding(1));
        let all_arrived = first.held.received_total == COMPLETE_STEPS.saturating_add(1)
            && second.held.received_total == COMPLETE_STEPS;
        if all_arrived && stale(&first) && stale(&second) {
            break;
        }
        assert!(started.elapsed() < DEADLINE, "no stale verdict within {DEADLINE:?}");
    }
    assert_eq!(first_reader.missing_sequences().expect("count"), 0);
    assert_eq!(second_reader.missing_sequences().expect("count"), 0);
    assert_eq!(counters(&mut second_reader), Vec::<u64>::new(), "nothing arrives after the kill");
    let health = safe::cycle_health(held);
    assert_eq!(health.callback_failures, 0, "no step of the consumer failed");

    safe::cycle_destroy(set, cycle).expect("cycle destroyed");
    assert!(safe::input_source_destroy(first_source).is_ok(), "source destroyed");
    assert!(safe::input_source_destroy(second_source).is_ok(), "source destroyed");
    drop(lease);
    safe::node_destroy(&node);
}
