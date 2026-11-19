//! The framework send stamp crosses the C++ and Rust SDKs in both directions.
//! The C++ peer `framework_stamp_peer` is built by `CMake` into `build/`.
#![allow(clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hypha_node::sdk::{BusNode, PublisherOptions, RawSample, RawSubscriber, SubscriberOptions};

const PEER: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../build/crates/hypha-cxx/framework_stamp_peer");
const DATA_TYPE: &str = "std_msgs/UInt8";
const DEFINITION: &str = "uint8 data\n";
/// The simulated clock value both sides use, far from wall time.
const SIM_STAMP: u64 = 1_700_000_000_123_456_789;
/// The explicit stamp set on the second frame of each direction.
const OVERRIDE_STAMP: u64 = 42_000_000_007;
const WAIT_LIMIT: Duration = Duration::from_secs(10);

fn wait_for_topic(bus: &BusNode, topic: &str, limit: Duration) {
    let started = Instant::now();
    loop {
        let listed = bus.list_topics().expect("the topic list");
        let open = listed.iter().any(|entry| entry.name == topic);
        if open {
            return;
        }
        let expired = started.elapsed() >= limit;
        assert!(!expired, "timed out after {limit:?} waiting for topic '{topic}'");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Kills the peer if the test panics before it has been waited for.
struct Peer(Option<Child>);

impl Peer {
    fn start(args: &[String]) -> Self {
        let path = PathBuf::from(PEER);
        assert!(
            path.is_file(),
            "the C++ peer is not built at {}; run: cmake --build build --target framework_stamp_peer",
            path.display()
        );
        let child = Command::new(&path).args(args).spawn().expect("start the C++ peer");
        Self(Some(child))
    }

    fn finish(mut self) -> ExitStatus {
        let mut child = self.0.take().expect("the peer is waited for once");
        let deadline = Instant::now().checked_add(WAIT_LIMIT).expect("deadline");
        loop {
            if let Some(status) = child.try_wait().expect("poll the C++ peer") {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the C++ peer did not finish within {WAIT_LIMIT:?}");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn unique_topic(label: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("UTC").as_nanos();
    format!("/framework_stamp_{label}_{}_{nanos:x}", std::process::id())
}

/// Collects the identities of the first two frames, or fails at the deadline.
fn receive_two(subscriber: &mut RawSubscriber) -> [RawSample; 2] {
    let mut identities = Vec::new();
    let deadline = Instant::now().checked_add(WAIT_LIMIT).expect("deadline");
    while identities.len() < 2 {
        if let Some(sample) = subscriber.try_receive().expect("receive") {
            identities.push(sample.to_raw_sample().expect("a frame from the peer has an identity"));
            continue;
        }
        assert!(Instant::now() < deadline, "received {} of 2 frames", identities.len());
        std::thread::sleep(Duration::from_millis(1));
    }
    let [first, second]: [RawSample; 2] = identities.try_into().expect("two frames");
    [first, second]
}

#[test]
fn framework_stamp_round_trips_between_cxx_and_rust() {
    let md5 = hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, DEFINITION).expect("md5");

    // C++ publishes with its simulated clock, then with an explicit stamp. Rust
    // receives both frames and reads each stamp from the frame header.
    {
        let bus = BusNode::new("framework_stamp_rust_receiver").expect("node");
        let topic = unique_topic("cxx_to_rust");
        let peer = Peer::start(&[
            "publish".into(),
            topic.clone(),
            md5.clone(),
            SIM_STAMP.to_string(),
            OVERRIDE_STAMP.to_string(),
        ]);
        // This subscriber declares no policy, so it waits for the topic before it subscribes.
        wait_for_topic(&bus, &topic, Duration::from_secs(30));
        let mut subscriber = bus
            .subscribe_raw_with_schema(
                &topic,
                DATA_TYPE,
                "",
                SubscriberOptions::default().keeping(8),
            )
            .expect("subscription");
        let [first, second] = receive_two(&mut subscriber);
        assert_eq!(first.framework_sent_nanos, SIM_STAMP, "the C++ clock stamp arrives in Rust");
        assert_eq!(second.framework_sent_nanos, OVERRIDE_STAMP, "the C++ override arrives in Rust");
        assert!(first.transport_sent_nanos > 0, "the host send time is still carried");
        assert!(peer.finish().success(), "the C++ publisher reported a failure");
    }

    // Rust publishes with its simulated clock, then with an explicit stamp. The
    // C++ peer checks both stamps on its side.
    {
        let bus = BusNode::new("framework_stamp_rust_publisher").expect("node");
        let topic = unique_topic("rust_to_cxx");
        hypha_node::sdk::clock::enable_sim_time(
            i64::try_from(SIM_STAMP).expect("sim stamp fits i64"),
        )
        .expect("enable sim time");
        let mut publisher = bus
            .publish_raw_configured(
                &topic,
                DATA_TYPE,
                &md5,
                DEFINITION,
                PublisherOptions::default(),
            )
            .expect("publisher");
        let peer = Peer::start(&[
            "receive".into(),
            topic,
            SIM_STAMP.to_string(),
            OVERRIDE_STAMP.to_string(),
        ]);
        let started = Instant::now();
        let mut reached = false;
        while !reached && started.elapsed() < WAIT_LIMIT {
            reached = publisher.min_subscriber_buffer().expect("subscriber buffers").is_some();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(reached, "the C++ subscriber connected in time");
        assert!(publisher.publish(&[7]), "the clock-stamped frame is sent");
        assert!(
            publisher.publish_at_framework_nanos(&[8], OVERRIDE_STAMP),
            "the overridden frame is sent"
        );
        let status = peer.finish();
        hypha_node::sdk::clock::disable_sim_time().expect("disable sim time");
        assert!(status.success(), "the C++ subscriber saw the wrong framework stamps");
    }
}
