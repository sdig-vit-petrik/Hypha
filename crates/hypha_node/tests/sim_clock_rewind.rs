//! A node on simulated time never rewinds in place (#130): a `/clock` sample lower than
//! the last one the node applied ends its `WaitSet` run with `ClockRewound`, whatever is
//! attached, in the lockstep and in an open-loop playback (`HYPHA_REPLAY_LOCKSTEP=0`).
#![allow(clippy::expect_used, reason = "a test fails on a refused setup step")]

use std::time::Duration;

use hypha_node::sdk::{BusNode, PublisherOptions, RawPublisher, StopReason, WaitSet};
use hypha_node::status::HyphaStatus;
use hypha_testkit::unique_name;

const CLOCK_MD5: &str = "a9c97c1d230cfc112e270351a944ee47";
const DATA_TYPE: &str = "std_msgs/Counter";
const DATA_MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const DATA_DEFINITION: &str = "uint32 value\n";
const SECOND: u64 = 1_000_000_000;
/// One run: long enough for the `WaitSet` to read a `/clock` sample already queued.
const TURN: Duration = Duration::from_millis(50);

/// A node on simulated time, in the lockstep or not, whose `WaitSet` holds one
/// subscription and nothing else, and a second node that publishes `/clock`.
struct Rig {
    // The node is leaked: a test process runs one test and ends with it.
    waitset: WaitSet<'static>,
    clock: RawPublisher,
    _clock_node: BusNode,
}

impl Rig {
    fn new(label: &str, lockstep: &str) -> Self {
        hypha_testkit::set_env("HYPHA_USE_SIM_TIME", "1");
        hypha_testkit::set_env("HYPHA_REPLAY_LOCKSTEP", lockstep);
        let node: &'static BusNode =
            Box::leak(Box::new(BusNode::new(&unique_name(label)).expect("node")));
        hypha_testkit::remove_env("HYPHA_USE_SIM_TIME");
        hypha_testkit::remove_env("HYPHA_REPLAY_LOCKSTEP");
        let clock_node = BusNode::new(&unique_name("clock")).expect("clock node");
        let clock = clock_node
            .publish_raw_configured(
                "/clock",
                "rosgraph_msgs/Clock",
                CLOCK_MD5,
                "",
                PublisherOptions::default(),
            )
            .expect("/clock publisher");
        let topic = format!("/{}", unique_name("data"));
        let _data = node
            .publish_raw_configured(
                &topic,
                DATA_TYPE,
                DATA_MD5,
                DATA_DEFINITION,
                PublisherOptions::default(),
            )
            .expect("data publisher");
        let subscriber = node.subscribe_raw(&topic, 8).expect("subscriber");
        let mut waitset = WaitSet::new(node).expect("waitset");
        waitset.attach_subscription(subscriber, |_| Ok(())).expect("attach");
        Self { waitset, clock, _clock_node: clock_node }
    }

    fn clock(&mut self, nanos: u64) {
        let secs = u32::try_from(nanos / SECOND).expect("ROS time");
        let subsec = u32::try_from(nanos % SECOND).expect("ROS time");
        let mut payload = [0_u8; 8];
        payload[..4].copy_from_slice(&secs.to_le_bytes());
        payload[4..].copy_from_slice(&subsec.to_le_bytes());
        assert!(self.clock.publish(&payload), "/clock is sent");
    }

    fn run(&mut self) -> hypha_node::sdk::RunResult {
        self.waitset.run_for(TURN)
    }
}

fn assert_rewound(result: &hypha_node::sdk::RunResult) {
    assert_eq!(result.reason, StopReason::RuntimeFailed, "{:?}", result.err());
    assert_eq!(result.status, HyphaStatus::ClockRewound as i32, "{:?}", result.err());
}

#[test]
fn an_open_loop_node_with_only_a_subscription_ends_its_run_on_a_backward_clock() {
    let mut rig = Rig::new("open_loop_rewind", "0");
    rig.clock(100 * SECOND);
    let first = rig.run();
    assert!(first.ok(), "{:?}", first.err());
    rig.clock(100 * SECOND);
    rig.clock(101 * SECOND);
    let forward = rig.run();
    assert!(forward.ok(), "a repeated and a later clock are no rewind: {:?}", forward.err());
    rig.clock(50 * SECOND);
    assert_rewound(&rig.run());
}

#[test]
fn a_lockstep_node_with_only_a_subscription_ends_its_run_on_a_backward_clock() {
    let mut rig = Rig::new("lockstep_rewind", "1");
    rig.clock(100 * SECOND);
    let first = rig.run();
    assert!(first.ok(), "{:?}", first.err());
    rig.clock(50 * SECOND);
    assert_rewound(&rig.run());
}
