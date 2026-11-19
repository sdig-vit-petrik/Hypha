//! Upstream hold on simulated time: a node runs a deadline only after its upstream reported it.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use hypha_core::message::DecodeOwned as _;
use hypha_msgs::hypha_msgs::ReplayProgress;

mod common;
use crate::common::emit_message;
use crate::common::replay::*;
use crate::common::supervision::bind_trigger;
use crate::common::{c_text, create_node, unique};
use hypha_ffi::waitset::*;
use hypha_ffi::*;

const MS: u64 = 1_000_000;
const SECOND: u64 = 1_000_000_000;
/// The first clock value of every case; the cycles anchor on it.
const BASE: u64 = SECOND;
const PERIOD: u64 = 10 * MS;
const PERIODS: u64 = 30;
/// Subscriber queue depth of every topic; it holds a whole case.
const DEPTH: usize = 32;
/// The identity of the chain topics. Every stage and the player share it.
const CHAIN_MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const CHAIN_TYPE: &str = "std_msgs/Counter";
/// The silence limit of a triggered stage's trigger.
const SILENCE: u64 = 5 * PERIOD;

/// What a step emits, and what it reads from its input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Emits the newest input byte plus 1000 times the step index.
    Relay,
    /// Emits the sum of the values it captured.
    Sum,
    /// Emits its own deadline, and reads the newest value captured.
    Echo,
}

/// One step as the test reads it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    scheduled: i64,
    /// The value the step read: the newest captured value, or the sum for `Sum`.
    /// `None` when it captured nothing.
    seen: Option<u64>,
}

struct Context {
    kind: Kind,
    steps: Mutex<Vec<Record>>,
}

unsafe fn captured(image: *const HyphaCycleImage) -> Vec<Vec<u8>> {
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) }, HyphaStatus::Ok);
    let mut payloads = Vec::new();
    for index in 0..binding.health.capture.sample_count {
        let mut view = HyphaCycleSampleView {
            payload: ptr::null(),
            payload_len: 0,
            transport_sent_nanos: 0,
            sequence: 0,
            publisher_id: 0,
            publisher_sequence: 0,
        };
        let status = unsafe { hypha_cycle_image_sample(image, 0, index, &raw mut view) };
        assert_eq!(status, HyphaStatus::Ok);
        payloads
            .push(unsafe { std::slice::from_raw_parts(view.payload, view.payload_len) }.to_vec());
    }
    payloads
}

fn word(payload: &[u8]) -> u64 {
    let bytes: [u8; 8] = payload.try_into().expect("an eight-byte value");
    u64::from_le_bytes(bytes)
}

extern "C" fn run_step(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let context = unsafe { &*context.cast::<Context>() };
    let scheduled = unsafe { (*time).scheduled_nanos };
    let payloads = unsafe { captured(image) };
    let (seen, emitted) = match context.kind {
        Kind::Relay => {
            let newest = payloads.last().and_then(|payload| payload.first().copied());
            let period = i64::try_from(PERIOD).unwrap();
            let index =
                u64::try_from((scheduled - i64::try_from(BASE).unwrap()) / period).unwrap_or(0);
            let emitted = u64::from(newest.unwrap_or(0)) + 1000 * index;
            (newest.map(u64::from), emitted)
        }
        Kind::Sum => {
            let values: Vec<u64> = payloads.iter().map(|payload| word(payload)).collect();
            let sum = values.iter().fold(0_u64, |total, value| total.saturating_add(*value));
            (if values.is_empty() { None } else { Some(sum) }, sum)
        }
        Kind::Echo => {
            let newest = payloads.last().map(|payload| word(payload));
            (newest, u64::try_from(scheduled).unwrap_or(0))
        }
    };
    context.steps.lock().expect("steps").push(Record { scheduled, seen });
    let status = unsafe { emit_message(emission, 0, &emitted.to_le_bytes()) };
    assert_eq!(status, HyphaStatus::Ok);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// One node with a cycle that reads `input` and writes `output`; the cycle owns the
/// output publisher.
struct Stage {
    name: String,
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    source: *mut HyphaInputSource,
    cycle: *mut HyphaCycle,
    context: Box<Context>,
    input_declarer: *mut HyphaPublisher,
}

impl Stage {
    fn new(label: &str, kind: Kind, input: &str, output: &str) -> Self {
        Self::build(label, kind, input, output, false)
    }

    /// A stage whose cycle runs on each sample of `input`, falling back after `SILENCE`.
    fn triggered(label: &str, kind: Kind, input: &str, output: &str) -> Self {
        Self::build(label, kind, input, output, true)
    }

    fn build(label: &str, kind: Kind, input: &str, output: &str, triggered: bool) -> Self {
        let name = unique(label);
        let silence_ms = i64::try_from(SILENCE / 1_000_000).unwrap();
        let node = create_sim_node_with_triggers(&name, &[silence_ms]);
        let waitset = unsafe { create_waitset(node) };
        // The input is created here by a publisher that declares the test policy, so the
        // subscriber may keep DEPTH samples. The player feeds the topic later, undeclared.
        let input_declarer = unsafe { open_declared_publisher(node, input, CHAIN_TYPE, CHAIN_MD5) };
        let subscriber = unsafe { open_subscriber(node, input, CHAIN_MD5, DEPTH) };
        let mut source = ptr::null_mut();
        let status = unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) };
        assert_eq!(status, HyphaStatus::Ok);
        let output = unsafe { open_declared_publisher(node, output, CHAIN_TYPE, CHAIN_MD5) };
        let context = Box::new(Context { kind, steps: Mutex::new(Vec::new()) });
        let label = c_text(&name);
        let mut cycle = ptr::null_mut();
        let status = unsafe {
            hypha_cycle_create(
                waitset,
                label.as_ptr(),
                label.as_bytes().len(),
                if triggered { 0 } else { PERIOD },
                Some(run_step),
                (&raw const *context).cast_mut().cast::<c_void>(),
                None,
                &raw mut cycle,
            )
        };
        assert_eq!(status, HyphaStatus::Ok);
        let bind = HyphaCycleBindingOptions {
            mode: HyphaCycleInputMode::Queued as u32,
            max_samples_per_iteration: DEPTH,
            optional: 0,
        };
        let mut index = 0_u32;
        let status =
            unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) };
        assert_eq!(status, HyphaStatus::Ok);
        if triggered {
            let silence_ms = i64::try_from(SILENCE / 1_000_000).unwrap();
            let bound = unsafe { bind_trigger(cycle, node, index, silence_ms) };
            assert_eq!(bound.status, HyphaStatus::Ok, "{}", bound.detail);
            assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
        }
        let mut out_index = 0_u32;
        let status = unsafe {
            hypha_cycle_bind_output(
                cycle,
                output,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut out_index,
            )
        };
        assert_eq!(status, HyphaStatus::Ok);
        assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
        Self { name, node, waitset, source, cycle, context, input_declarer }
    }

    fn steps(&self) -> Vec<Record> {
        self.context.steps.lock().expect("steps").clone()
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        unsafe {
            hypha_cycle_destroy(self.cycle);
            hypha_input_source_destroy(self.source);
            hypha_publisher_destroy(self.input_declarer);
            hypha_node_destroy(self.node);
        }
    }
}

/// A player node, which publishes the input and `/clock` and publishes no progress, and
/// the stages under test.
struct Bus {
    _serial: MutexGuard<'static, ()>,
    player: *mut HyphaNode,
    clock: *mut HyphaPublisher,
    clock_reader: *mut HyphaSubscriber,
    progress: *mut HyphaSubscriber,
    inputs: Vec<*mut HyphaPublisher>,
    stages: Vec<Stage>,
    /// The newest report of each node: its clock and the send time of its sample.
    latest: BTreeMap<String, (u64, u64)>,
}

impl Bus {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let player = create_node(&unique("upstream_player"));
        Self {
            _serial: serial,
            player,
            clock: unsafe { clock_publisher(player) },
            clock_reader: unsafe { open_subscriber(player, "/clock", CLOCK_MD5, 8) },
            progress: unsafe {
                open_subscriber(player, "/clock_progress", ReplayProgress::SCHEMA_HASH, DEPTH)
            },
            inputs: Vec::new(),
            stages: Vec::new(),
            latest: BTreeMap::new(),
        }
    }

    fn add_stage(&mut self, label: &str, kind: Kind, input: &str, output: &str) -> usize {
        self.stages.push(Stage::new(label, kind, input, output));
        self.stages.len() - 1
    }

    fn add_triggered(&mut self, label: &str, kind: Kind, input: &str, output: &str) -> usize {
        self.stages.push(Stage::triggered(label, kind, input, output));
        self.stages.len() - 1
    }

    fn stage(&self, index: usize) -> &Stage {
        self.stages.get(index).expect("a stage of this case")
    }

    /// Publish one input on `topic`, stamped with each value's log time `PERIOD * index`.
    fn feed(&mut self, topic: &str, count: u64) {
        let publisher = unsafe { open_publisher(self.player, topic, CHAIN_TYPE, CHAIN_MD5, "") };
        for index in 0..count {
            let stamp = BASE + index * PERIOD;
            let value = u8::try_from(index + 1).expect("the input fits a byte");
            unsafe { send_stamped(publisher, value, i64::try_from(stamp).unwrap()) };
        }
        self.inputs.push(publisher);
    }

    /// Publish `/clock` at `nanos`, and return the send time of that sample.
    fn tick(&self, nanos: u64) -> u64 {
        unsafe { publish_clock(self.clock, nanos) };
        let (_, identity) =
            unsafe { receive_frame(self.clock_reader) }.expect("the clock sample is queued");
        identity.transport_sent_nanos
    }

    fn drain_progress(&mut self) {
        while let Some(payload) = unsafe { receive_payload(self.progress) } {
            let report = ReplayProgress::decode_from(&payload).expect("a progress report decodes");
            self.latest.insert(report.node, (report.clock_nanos, report.clock_sent_nanos));
        }
    }

    /// True when `name` reported a clock of at least `nanos` for a sample sent at or after
    /// `floor`, the send time of the sample that the case publishes for `nanos`.
    fn acked(&self, name: &str, nanos: u64, floor: u64) -> bool {
        self.latest.get(name).is_some_and(|&(clock, sent)| clock >= nanos && sent >= floor)
    }

    /// Publish `nanos` and run the stages in `order` until every stage acknowledges it.
    fn advance(&mut self, nanos: u64, order: &[usize]) {
        let floor = self.tick(nanos);
        let started = Instant::now();
        loop {
            for &index in order {
                run_for(self.stage(index).waitset, 2 * MS);
            }
            self.drain_progress();
            let all_acked = self.stages.iter().all(|stage| self.acked(&stage.name, nanos, floor));
            if all_acked {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the stages did not acknowledge clock {nanos} ns in two seconds"
            );
        }
    }

    /// A case: the anchor, then the clock in steps of `chunk` periods.
    fn replay(&mut self, chunk: u64, order: &[usize]) {
        self.advance(BASE, order);
        let mut at = 0;
        while at < PERIODS {
            at = (at + chunk).min(PERIODS);
            self.advance(BASE + at * PERIOD, order);
        }
    }

    fn observed(observer: *mut HyphaSubscriber) -> Vec<(u64, u64)> {
        let mut seen = Vec::new();
        while let Some((payload, identity)) = unsafe { receive_frame(observer) } {
            seen.push((word(&payload), identity.framework_sent_nanos));
        }
        seen
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        self.stages.clear();
        unsafe {
            for publisher in &self.inputs {
                hypha_publisher_destroy(*publisher);
            }
            hypha_subscriber_destroy(self.progress);
            hypha_subscriber_destroy(self.clock_reader);
            hypha_publisher_destroy(self.clock);
            hypha_node_destroy(self.player);
        }
        // Simulated time belongs to the process and outlives its nodes: the next bus of
        // this process starts on a fresh clock.
        let _status = hypha_clock_disable_sim_time();
    }
}

/// The output of a chain `ch_a -> ch_b`; `ch_b_first` runs `ch_b` before `ch_a`.
fn chain_output(chunk: u64, ch_b_first: bool) -> Vec<(u64, u64)> {
    let mut bus = Bus::new();
    let a = bus.add_stage("ch_a", Kind::Relay, "/ch_in", "/ch_mid");
    let b = bus.add_stage("ch_b", Kind::Sum, "/ch_mid", "/ch_out");
    let observer = unsafe { open_subscriber(bus.player, "/ch_out", CHAIN_MD5, DEPTH) };
    bus.feed("/ch_in", PERIODS);
    let order = if ch_b_first { [b, a] } else { [a, b] };
    bus.replay(chunk, &order);
    let seen = Bus::observed(observer);
    unsafe { hypha_subscriber_destroy(observer) };
    seen
}

#[test]
fn two_chained_nodes_give_the_same_output_in_any_turn_order() {
    let one_period_per_chunk = chain_output(1, false);
    let b_first = chain_output(10, true);
    let a_first = chain_output(10, false);
    assert_eq!(one_period_per_chunk.len(), usize::try_from(PERIODS).unwrap());
    assert_eq!(b_first, one_period_per_chunk, "b before a changed the output");
    assert_eq!(a_first, one_period_per_chunk, "a before b changed the output");
}
#[test]
fn a_feedback_loop_between_two_nodes_does_not_deadlock() {
    let mut bus = Bus::new();
    let a = bus.add_stage("fb_a", Kind::Echo, "/fb_b_out", "/fb_a_out");
    let b = bus.add_stage("fb_b", Kind::Echo, "/fb_a_out", "/fb_b_out");
    bus.advance(BASE, &[a, b]);
    // One chunk of twenty periods: each node needs the other's progress for every step.
    bus.advance(BASE + 20 * PERIOD, &[a, b]);

    for index in [a, b] {
        let mut steps = bus.stage(index).steps();
        steps.sort_by_key(|step| step.scheduled);
        let expected: Vec<i64> =
            (1..=20).map(|periods| i64::try_from(BASE + periods * PERIOD).unwrap()).collect();
        let scheduled: Vec<i64> = steps.iter().map(|step| step.scheduled).collect();
        assert_eq!(scheduled, expected, "{} stepped every deadline once", bus.stage(index).name);
        for pair in steps.windows(2) {
            let [previous, step] = pair else { continue };
            let seen = step.seen.expect("a step after the first sees the other's output");
            assert_eq!(seen, u64::try_from(previous.scheduled).unwrap(), "{step:?}");
        }
    }
}

#[test]
fn a_node_whose_upstream_stops_reporting_holds_without_a_time_limit() {
    let mut bus = Bus::new();
    let up = bus.add_stage("up", Kind::Echo, "/up_in", "/up_out");
    let down = bus.add_stage("down", Kind::Echo, "/up_out", "/down_out");
    // Both read the anchor, and `up` reports it. Then `up` is never run again.
    let floor = bus.tick(BASE);
    for _ in 0..50 {
        run_for(bus.stage(up).waitset, 2 * MS);
        run_for(bus.stage(down).waitset, 2 * MS);
    }
    bus.drain_progress();
    assert!(bus.acked(&bus.stage(up).name, BASE, floor), "the upstream reported the anchor");
    bus.tick(BASE + 20 * PERIOD);

    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    // The player's --timeout names a silent participant; the node itself only holds.
    let status = unsafe { hypha_waitset_run(bus.stage(down).waitset, 500 * MS, &raw mut report) };
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(report.reason, hypha_ffi::waitset::HYPHA_RUN_REASON_DEADLINE, "the run only held");
    assert_eq!(report.status, HyphaStatus::Ok as i32);
    assert!(bus.stage(down).steps().is_empty(), "no deadline of the held node ran");
}

#[test]
fn an_input_published_only_by_the_player_never_holds() {
    let mut bus = Bus::new();
    let solo = bus.add_stage("solo", Kind::Relay, "/solo_in", "/solo_out");
    bus.feed("/solo_in", PERIODS);
    for periods in [0, 20] {
        let nanos = BASE + periods * PERIOD;
        let floor = bus.tick(nanos);
        // One run of the node, with no other node running: its progress arrives at once.
        run_for(bus.stage(solo).waitset, 2 * MS);
        bus.drain_progress();
        assert!(bus.acked(&bus.stage(solo).name, nanos, floor), "acked {nanos} ns in one run");
    }
}

/// The output of `ch_a -> ch_b` where `ch_b` runs on each sample of `ch_a`, and the
/// deadlines `ch_a` stepped.
fn triggered_chain_output(chunk: u64, ch_b_first: bool) -> (Vec<(u64, u64)>, Vec<i64>) {
    let mut bus = Bus::new();
    let a = bus.add_stage("tch_a", Kind::Relay, "/tch_in", "/tch_mid");
    let b = bus.add_triggered("tch_b", Kind::Sum, "/tch_mid", "/tch_out");
    let observer = unsafe { open_subscriber(bus.player, "/tch_out", CHAIN_MD5, DEPTH) };
    bus.feed("/tch_in", PERIODS);
    let order = if ch_b_first { [b, a] } else { [a, b] };
    bus.replay(chunk, &order);
    let seen = Bus::observed(observer);
    unsafe { hypha_subscriber_destroy(observer) };
    let deadlines = bus.stage(a).steps().iter().map(|step| step.scheduled).collect();
    (seen, deadlines)
}

#[test]
fn a_triggered_chain_gives_the_same_output_in_any_turn_order() {
    let (reference, deadlines) = triggered_chain_output(1, false);
    assert!(!reference.is_empty(), "the triggered stage produced output");
    let stamps: Vec<i64> =
        reference.iter().map(|&(_, stamp)| i64::try_from(stamp).unwrap()).collect();
    assert_eq!(stamps, deadlines, "each output carries the stamp of the sample that triggered it");
    for chunk in [1, 3, 7] {
        for ch_b_first in [false, true] {
            let (seen, _) = triggered_chain_output(chunk, ch_b_first);
            assert_eq!(seen, reference, "chunk {chunk}, ch_b first {ch_b_first}");
        }
    }
}

#[test]
fn a_feedback_loop_of_a_triggered_and_a_rate_cycle_does_not_deadlock() {
    let mut bus = Bus::new();
    let rate = bus.add_stage("fbt_rate", Kind::Echo, "/fbt_t_out", "/fbt_r_out");
    let triggered = bus.add_triggered("fbt_trig", Kind::Echo, "/fbt_r_out", "/fbt_t_out");
    bus.replay(1, &[rate, triggered]);
    let rate_steps = bus.stage(rate).steps();
    let triggered_steps = bus.stage(triggered).steps();
    assert_eq!(rate_steps.len(), usize::try_from(PERIODS).unwrap(), "{rate_steps:?}");
    let trigger_times: Vec<i64> = triggered_steps.iter().map(|step| step.scheduled).collect();
    let rate_times: Vec<i64> = rate_steps.iter().map(|step| step.scheduled).collect();
    assert_eq!(trigger_times, rate_times, "one triggered step per rate output");
}

#[test]
fn a_loop_of_two_triggered_cycles_holds_without_failing() {
    // Two triggered cycles that wait on each other hold for good under replay: accepted
    // (sim time is development only); the player's --timeout ends such a run.
    let mut bus = Bus::new();
    let a = bus.add_triggered("tloop_a", Kind::Echo, "/tloop_b_out", "/tloop_a_out");
    let b = bus.add_triggered("tloop_b", Kind::Echo, "/tloop_a_out", "/tloop_b_out");
    bus.feed("/tloop_b_out", 1);
    bus.tick(BASE);
    for _ in 0..10 {
        for index in [a, b] {
            let mut report =
                HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
            let waitset = bus.stage(index).waitset;
            let status = unsafe { hypha_waitset_run(waitset, 50 * MS, &raw mut report) };
            assert_eq!(status, HyphaStatus::Ok);
            assert_eq!(report.status, HyphaStatus::Ok as i32, "a hold never fails the run");
        }
    }
    assert!(bus.stage(a).steps().is_empty(), "the held trigger never ran");
}

fn fallback_times(chunk: u64) -> Vec<i64> {
    let mut bus = Bus::new();
    let stage = bus.add_triggered("tfall", Kind::Relay, "/tfall_in", "/tfall_out");
    bus.feed("/tfall_in", 10);
    bus.replay(chunk, &[stage]);
    bus.stage(stage).steps().iter().map(|step| step.scheduled).collect()
}

#[test]
fn a_triggered_cycle_falls_back_at_deterministic_times() {
    let base = i64::try_from(BASE).unwrap();
    let period = i64::try_from(PERIOD).unwrap();
    let silence = i64::try_from(SILENCE).unwrap();
    let last_trigger = base + 9 * period;
    let mut expected: Vec<i64> = (0..10).map(|index| base + index * period).collect();
    let end = base + i64::try_from(PERIODS).unwrap() * period;
    expected.extend((1..).map(|k| last_trigger + k * silence).take_while(|time| *time <= end));
    for chunk in [1, 4] {
        assert_eq!(fallback_times(chunk), expected, "chunk {chunk}");
    }
}

/// A `/clock` that goes back ends the run with `ClockRewound`, so that the process restarts.
#[test]
fn a_rewind_of_the_bus_clock_ends_the_run_of_a_triggered_cycle() {
    let mut bus = Bus::new();
    let stage = bus.add_triggered("trewind", Kind::Relay, "/trewind_in", "/trewind_out");
    bus.advance(BASE + 10 * PERIOD, &[stage]);
    bus.tick(BASE + 2 * PERIOD);
    let status = run_status(bus.stage(stage).waitset, 20 * MS);
    assert_eq!(status, HyphaStatus::ClockRewound as i32);
}
