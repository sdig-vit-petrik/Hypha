//! `HYPHA_REPLAY_LOCKSTEP=0` turns the replay lockstep off for a node on simulated time
//! (#102): its plain callbacks run as their samples arrive, and a replay cycle that lost
//! input samples warns and keeps running. Each lockstep case beside it shows the default.
#![allow(clippy::expect_used, reason = "a test fails on a refused setup step")]
#![allow(clippy::arithmetic_side_effects, reason = "test times are small constants")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hypha_node::cycle::{
    CycleSnapshots, CycleStepHandler, HyphaCycleBindingOptions, HyphaCycleEmission,
    HyphaCycleImage, HyphaCycleInputMode, HyphaCycleTime,
};
use hypha_node::safe;
use hypha_node::sdk::{BusNode, PublisherOptions, RawPublisher, WaitSet};
use hypha_node::status::HyphaStatus;
use hypha_node::waitset::HyphaCallbackResult;
use hypha_testkit::unique_name;

const CLOCK_MD5: &str = "a9c97c1d230cfc112e270351a944ee47";
const DATA_TYPE: &str = "std_msgs/UInt8";
const DATA_MD5: &str = "7c8164229e7d2c17eb95e9231617fdee";
const DATA_DEFINITION: &str = "uint8 data\n";
const SECOND: u64 = 1_000_000_000;
const MILLISECOND: u64 = 1_000_000;
/// 2026-10-08T12:00:00Z: the simulated time the tests start at.
const BASE: u64 = 1_791_460_800 * SECOND;
/// One run: long enough for the `WaitSet` to read the samples already queued.
const TURN: Duration = Duration::from_millis(50);

/// Create a node with `create` on simulated time, with the lockstep `lockstep` (`None`:
/// the variable unset, the default). The variables are removed again, so the feeder
/// that the test creates next is on host time and takes no part in the replay.
fn on_simulated_time<T>(lockstep: Option<&str>, create: impl FnOnce() -> T) -> T {
    hypha_testkit::set_env("HYPHA_USE_SIM_TIME", "1");
    match lockstep {
        Some(value) => hypha_testkit::set_env("HYPHA_REPLAY_LOCKSTEP", value),
        None => hypha_testkit::remove_env("HYPHA_REPLAY_LOCKSTEP"),
    }
    let created = create();
    hypha_testkit::remove_env("HYPHA_USE_SIM_TIME");
    hypha_testkit::remove_env("HYPHA_REPLAY_LOCKSTEP");
    created
}

/// A second node, on host time: the `/clock` publisher and the data publisher.
struct Feeder {
    clock: RawPublisher,
    data: RawPublisher,
    topic: String,
    _node: BusNode,
}

impl Feeder {
    fn new() -> Self {
        let node = BusNode::new(&unique_name("feeder")).expect("feeder node");
        let clock = node
            .publish_raw_configured(
                "/clock",
                "rosgraph_msgs/Clock",
                CLOCK_MD5,
                "",
                PublisherOptions::default(),
            )
            .expect("/clock publisher");
        let topic = format!("/{}", unique_name("data"));
        let data = node
            .publish_raw_configured(
                &topic,
                DATA_TYPE,
                DATA_MD5,
                DATA_DEFINITION,
                PublisherOptions::default(),
            )
            .expect("data publisher");
        Self { clock, data, topic, _node: node }
    }

    fn clock(&mut self, nanos: u64) {
        let secs = u32::try_from(nanos / SECOND).expect("ROS time");
        let subsec = u32::try_from(nanos % SECOND).expect("ROS time");
        let mut payload = [0_u8; 8];
        payload[..4].copy_from_slice(&secs.to_le_bytes());
        payload[4..].copy_from_slice(&subsec.to_le_bytes());
        assert!(self.clock.publish(&payload), "/clock is sent");
    }

    fn send(&mut self, value: u8, stamp: u64) {
        assert!(self.data.publish_at_framework_nanos(&[value], stamp), "a sample is sent");
    }
}

/// Run a node with one plain subscription: the clock stands at `BASE`, and one sample
/// stamped ten seconds later is queued. Returns how many callbacks ran.
fn callbacks_run_for_a_sample_ahead_of_the_clock(lockstep: Option<&str>) -> usize {
    let node =
        on_simulated_time(lockstep, || BusNode::new(&unique_name("callbacks"))).expect("node");
    let mut feeder = Feeder::new();
    let subscriber = node.subscribe_raw(&feeder.topic, 8).expect("subscriber");
    let ran = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&ran);
    let mut waitset = WaitSet::new(&node).expect("waitset");
    waitset
        .attach_subscription(subscriber, move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
        .expect("attach");
    feeder.clock(BASE);
    let first = waitset.run_for(TURN);
    assert!(first.ok(), "{:?}", first.err());
    feeder.send(1, BASE + 10 * SECOND);
    let second = waitset.run_for(TURN);
    assert!(second.ok(), "{:?}", second.err());
    ran.load(Ordering::Relaxed)
}

#[test]
fn an_open_loop_callback_runs_as_its_sample_arrives() {
    assert_eq!(callbacks_run_for_a_sample_ahead_of_the_clock(Some("0")), 1);
}

#[test]
fn a_lockstep_callback_waits_until_the_clock_reaches_its_sample() {
    assert_eq!(callbacks_run_for_a_sample_ahead_of_the_clock(None), 0);
}

/// Counts the steps of a cycle.
struct CountSteps(Arc<AtomicUsize>);

impl CycleStepHandler for CountSteps {
    fn step(
        &mut self,
        _image: *const HyphaCycleImage,
        _time: *const HyphaCycleTime,
        _emission: *mut HyphaCycleEmission,
    ) -> HyphaCallbackResult {
        self.0.fetch_add(1, Ordering::Relaxed);
        HyphaCallbackResult::continue_run()
    }
}

/// What one replay cycle's runs showed.
struct Cycled {
    /// The status the last run ended with.
    status: i32,
    /// The steps the cycle ran in all.
    steps: usize,
    /// The samples its input counted as lost.
    lost: u64,
}

/// Who feeds the cycle's input.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Upstream {
    /// A node on host time, which takes no part in the replay.
    None,
    /// A node in the lockstep that never reports its progress.
    Silent,
}

/// A node on simulated time, with the lockstep `lockstep`, that runs a 20 ms replay
/// cycle over one four-deep queued input. `drive` feeds it and runs its `WaitSet`, and
/// returns the report of the last run.
fn run_replay_cycle(
    lockstep: Option<&str>,
    upstream: Upstream,
    drive: impl FnOnce(&mut Feeder, &safe::HyphaWaitSet, &AtomicUsize) -> safe::HyphaWaitSetRunReport,
) -> Cycled {
    let node =
        on_simulated_time(lockstep, || safe::node_create(&unique_name("cycled"), None, None))
            .expect("node");
    let mut feeder = match upstream {
        Upstream::None => Feeder::new(),
        Upstream::Silent => on_simulated_time(Some("1"), Feeder::new),
    };
    let identity = safe::SubscriberIdentity {
        data_type: DATA_TYPE,
        expected_md5: Some(DATA_MD5),
        definition: DATA_DEFINITION,
    };
    let settings = safe::SubscriberSettings {
        depth: 4,
        no_publisher_warning_after_nanos: 0,
        request: hypha_core::port::QosRequest::Undeclared,
    };
    let subscriber =
        safe::subscribe_typed(&node, &feeder.topic, &identity, settings).expect("subscriber");
    let lease = safe::node_default_waitset(&node).expect("waitset");
    let set = lease.waitset().expect("the lent waitset");
    let source = safe::input_source_create(set, Some(subscriber)).map_err(|(error, _)| error);
    let source = source.expect("input source");
    let steps = Arc::new(AtomicUsize::new(0));
    let handler = CountSteps(Arc::clone(&steps));
    let snapshots = Arc::new(std::sync::OnceLock::new());
    let kept = Arc::clone(&snapshots);
    let adopt = move |_, held: &Arc<CycleSnapshots>| -> Box<dyn CycleStepHandler> {
        kept.get_or_init(|| Arc::clone(held));
        Box::new(handler)
    };
    let cycle =
        safe::cycle_create(set, "cycled", 20 * MILLISECOND, Some(adopt)).expect("cycle created");
    let bind = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: 64,
        optional: 0,
    };
    safe::cycle_bind_input(set, cycle, &source, &bind).expect("input bound");
    safe::cycle_commit(set, cycle).expect("cycle committed");

    let report = drive(&mut feeder, set, &steps);
    let binding = snapshots.get().map(|held| safe::cycle_binding(held, 0));
    let lost = binding.expect("the cycle's snapshots").expect("binding 0").health.overrun_total;
    safe::cycle_destroy(set, cycle).expect("cycle destroyed");
    assert!(safe::input_source_destroy(source).is_ok(), "source destroyed");
    drop(lease);
    safe::node_destroy(&node);
    Cycled { status: report.status, steps: steps.load(Ordering::Relaxed), lost }
}

/// The first clock value anchors the grid; one sample, which the step at 20 ms takes,
/// gives the transport a sequence to compare a burst with.
fn anchor(feeder: &mut Feeder, set: &safe::HyphaWaitSet) {
    feeder.clock(BASE);
    let anchored = safe::waitset_run(set, Some(TURN)).expect("the anchoring run");
    assert_eq!(anchored.status, 0, "the anchoring run fails nothing");
    feeder.send(0, BASE + MILLISECOND);
    feeder.clock(BASE + 25 * MILLISECOND);
    let first = safe::waitset_run(set, Some(TURN)).expect("the first run");
    assert_eq!(first.status, 0, "the first run fails nothing");
}

/// A burst of sixteen samples overflows the four-deep queue before the clock reaches
/// 200 ms.
fn after_a_replay_loss(lockstep: Option<&str>) -> Cycled {
    run_replay_cycle(lockstep, Upstream::None, |feeder, set, steps| {
        anchor(feeder, set);
        assert_eq!(steps.load(Ordering::Relaxed), 1, "the step at 20 ms ran");
        // Stamped after the deadlines that have run: the queue keeps the last four.
        for value in 1..=16_u8 {
            feeder.send(value, BASE + 30 * MILLISECOND + 10 * MILLISECOND * u64::from(value));
        }
        feeder.clock(BASE + 200 * MILLISECOND);
        safe::waitset_run(set, Some(TURN)).expect("the lossy run")
    })
}

/// The input's publisher is in the lockstep and never reports: a cycle in the lockstep
/// holds each deadline for it.
fn with_a_silent_upstream(lockstep: Option<&str>) -> Cycled {
    run_replay_cycle(lockstep, Upstream::Silent, |feeder, set, _| {
        feeder.clock(BASE);
        let anchored = safe::waitset_run(set, Some(TURN)).expect("the anchoring run");
        assert_eq!(anchored.status, 0, "the anchoring run fails nothing");
        feeder.send(0, BASE + MILLISECOND);
        feeder.clock(BASE + 100 * MILLISECOND);
        safe::waitset_run(set, Some(TURN)).expect("the run")
    })
}

#[test]
fn an_open_loop_replay_cycle_holds_no_deadline_for_a_silent_upstream() {
    let cycled = with_a_silent_upstream(Some("0"));
    assert_eq!(cycled.status, 0, "{}", cycled.status);
    assert_eq!(cycled.steps, 5, "the deadlines 20 ms to 100 ms ran");
}

#[test]
fn a_lockstep_replay_cycle_holds_each_deadline_for_a_silent_upstream() {
    let cycled = with_a_silent_upstream(None);
    assert_eq!(cycled.status, 0, "a held deadline fails nothing");
    assert_eq!(cycled.steps, 0, "every deadline waits for the upstream's report");
}

#[test]
fn an_open_loop_replay_cycle_warns_of_a_lost_sample_and_keeps_running() {
    let lossy = after_a_replay_loss(Some("false"));
    assert_eq!(lossy.status, 0, "a loss outside the lockstep fails no run");
    assert_eq!(lossy.steps, 10, "every deadline up to the clock ran a step");
    assert!(lossy.lost > 0, "the loss is counted, never silent");
}

#[test]
fn a_lockstep_replay_cycle_fails_its_run_on_a_lost_sample() {
    let lossy = after_a_replay_loss(None);
    assert_eq!(lossy.status, HyphaStatus::SequenceGap as i32, "a lossy lockstep step fails");
    assert!(lossy.lost > 0, "the loss is counted");
}

#[test]
fn a_lockstep_value_that_is_not_one_of_the_four_refuses_the_node() {
    let refused = on_simulated_time(Some("off"), || BusNode::new(&unique_name("refused")));
    assert!(refused.is_err(), "'off' is not a lockstep value");
    let kept = on_simulated_time(Some("1"), || BusNode::new(&unique_name("lockstep")));
    assert!(kept.is_ok(), "'1' keeps the lockstep");
}
