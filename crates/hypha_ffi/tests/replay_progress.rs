//! Replay progress on one node that runs on the bus clock. Every case holds [`SERIAL`].
#![allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use hypha_core::message::DecodeOwned as _;
use hypha_msgs::hypha_msgs::ReplayProgress;

mod common;
use crate::common::replay::*;
use crate::common::{c_text, unique};
use hypha_ffi::waitset::*;
use hypha_ffi::*;

static SERIAL: Mutex<()> = Mutex::new(());

const MS: u64 = 1_000_000;
const SECOND: u64 = 1_000_000_000;
/// The first clock value of every case; the cycles anchor on it.
const BASE: u64 = SECOND;
const PERIOD_NANOS: u64 = 20 * MS;

/// A node on simulated time, its `/clock` publisher, a reader of `/clock_progress`,
/// and one `WaitSet` for the cycles.
struct Bus {
    _serial: MutexGuard<'static, ()>,
    name: String,
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    clock: *mut HyphaPublisher,
    progress: *mut HyphaSubscriber,
    cycles: Vec<*mut HyphaCycle>,
    /// Each recorder keeps its address: its cycle holds it as the step context.
    #[allow(clippy::vec_box, reason = "a cycle holds the address of its recorder")]
    recorders: Vec<Box<Recorder>>,
}

impl Bus {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let name = unique("replay_progress");
        let node = create_sim_node(&name);
        let waitset = unsafe { create_waitset(node) };
        let clock = unsafe { clock_publisher(node) };
        let progress =
            unsafe { open_subscriber(node, "/clock_progress", ReplayProgress::SCHEMA_HASH, 8) };
        Self {
            _serial: serial,
            name,
            node,
            waitset,
            clock,
            progress,
            cycles: Vec::new(),
            recorders: Vec::new(),
        }
    }

    /// Commit a 50 Hz recorded cycle on the bus `WaitSet`. Returns its index.
    fn add_cycle(&mut self) -> usize {
        let recorder = Box::new(Recorder::new(Duration::ZERO));
        let cycle = unsafe { commit_cycle(self.waitset, &recorder) };
        self.cycles.push(cycle);
        self.recorders.push(recorder);
        self.recorders.len() - 1
    }

    fn steps(&self, index: usize) -> Vec<(i64, u64)> {
        let recorder = self.recorders.get(index).expect("a recorded cycle");
        recorder.steps().iter().map(|step| (step.scheduled, step.missed)).collect()
    }

    fn clock(&self, nanos: u64) {
        unsafe { publish_clock(self.clock, nanos) };
    }

    /// Run the bus `WaitSet` for 50 ms, which covers one notification and its turn.
    fn run(&self) {
        run_for(self.waitset, 50 * MS);
    }

    fn progress(&self) -> Vec<ReplayProgress> {
        let mut reports = Vec::new();
        while let Some(payload) = unsafe { receive_payload(self.progress) } {
            reports.push(ReplayProgress::decode_from(&payload).expect("a progress report decodes"));
        }
        reports
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        unsafe {
            for cycle in &self.cycles {
                assert_eq!(hypha_cycle_destroy(*cycle), HyphaStatus::Ok);
            }
            hypha_subscriber_destroy(self.progress);
            hypha_publisher_destroy(self.clock);
            hypha_node_destroy(self.node);
        }
        assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
    }
}

unsafe fn commit_cycle(waitset: *mut HyphaWaitSet, recorder: &Recorder) -> *mut HyphaCycle {
    let label = c_text(&unique("replay_cycle"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                label.as_ptr(),
                label.as_bytes().len(),
                PERIOD_NANOS,
                Some(record_step),
                ptr::from_ref(recorder).cast_mut().cast::<c_void>(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    cycle
}

fn only(reports: Vec<ReplayProgress>) -> ReplayProgress {
    let [report] = <[ReplayProgress; 1]>::try_from(reports).expect("exactly one report");
    report
}

/// The transport send time of the next frame that `reader` takes.
fn frame_sent_nanos(reader: *mut HyphaSubscriber) -> u64 {
    let (_, identity) = unsafe { receive_frame(reader) }.expect("the clock frame is queued");
    identity.transport_sent_nanos
}

#[test]
fn each_clock_sample_is_reported_with_its_clock_and_send_time() {
    let mut bus = Bus::new();
    let clock_reader = unsafe { open_subscriber(bus.node, "/clock", CLOCK_MD5, 8) };
    bus.add_cycle();
    bus.clock(BASE);
    bus.run();
    let anchor = only(bus.progress());
    assert_eq!(anchor.node, bus.name);
    assert_eq!(anchor.clock_nanos, BASE);
    // The send time is the one the transport stamped on the very frame the node read.
    assert_eq!(anchor.clock_sent_nanos, frame_sent_nanos(clock_reader));

    bus.clock(BASE + 50 * MS);
    bus.run();
    let next = only(bus.progress());
    assert_eq!(next.clock_nanos, BASE + 50 * MS);
    assert_eq!(next.clock_sent_nanos, frame_sent_nanos(clock_reader));
    assert!(next.clock_sent_nanos > anchor.clock_sent_nanos);
    // The jump expired the deadlines at 1.02 and 1.04, one step each.
    assert_eq!(bus.steps(0), [(1_020_000_000, 0), (1_040_000_000, 0)]);
    unsafe { hypha_subscriber_destroy(clock_reader) };
}

#[test]
fn republishing_the_same_clock_value_is_acknowledged_again() {
    let mut bus = Bus::new();
    bus.add_cycle();
    bus.clock(BASE);
    bus.run();
    let first = only(bus.progress());
    bus.clock(BASE);
    bus.run();
    let again = only(bus.progress());
    assert_eq!(again.clock_nanos, BASE, "the same value is reported again");
    assert!(again.clock_sent_nanos > first.clock_sent_nanos);
}

#[test]
fn a_chunk_that_expires_no_deadline_is_still_acknowledged() {
    let mut bus = Bus::new();
    bus.add_cycle();
    bus.clock(BASE);
    bus.run();
    bus.progress();
    bus.clock(BASE + 5 * MS);
    bus.run();
    let ack = only(bus.progress());
    assert_eq!(ack.clock_nanos, BASE + 5 * MS);
    assert!(bus.steps(0).is_empty(), "no deadline expired before 1.02");
}

#[test]
fn a_rewind_of_the_bus_clock_ends_the_run_of_a_replay_cycle() {
    let mut bus = Bus::new();
    bus.add_cycle();
    bus.clock(BASE);
    bus.run();
    bus.clock(BASE + 100 * MS);
    bus.run();
    bus.progress();
    bus.clock(BASE + 50 * MS);
    assert_eq!(run_status(bus.waitset, 50 * MS), HyphaStatus::ClockRewound as i32);
    assert!(bus.progress().is_empty(), "the rewound sample is not acknowledged");
}

#[test]
fn a_forward_clock_step_without_an_announcement_runs_each_period() {
    let mut bus = Bus::new();
    let cycle = bus.add_cycle();
    bus.clock(BASE);
    bus.run();
    bus.progress();
    bus.clock(BASE + 500 * MS);
    bus.run();
    // The deadlines 1.02 to 1.50 are 25 periods, and each one runs its own step.
    let steps = bus.steps(cycle);
    assert_eq!(steps.len(), 25);
    assert!(steps.iter().all(|&(_, missed)| missed == 0), "{steps:?}");
    assert_eq!(steps.first(), Some(&(1_020_000_000, 0)));
    assert_eq!(steps.last(), Some(&(1_500_000_000, 0)));
    assert_eq!(only(bus.progress()).clock_nanos, BASE + 500 * MS);
}

/// The output of [`emit_step`]. It also reads the progress queued so far, so a step can
/// see whether its own progress came first.
struct Emitter {
    publisher: *mut HyphaPublisher,
    progress: *mut HyphaSubscriber,
    /// Steps whose progress was already queued when they sent their output.
    early: AtomicUsize,
}

extern "C" fn emit_step(
    _image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let emitter = unsafe { &*context.cast::<Emitter>() };
    let scheduled = unsafe { (*time).scheduled_nanos };
    while let Some(payload) = unsafe { receive_payload(emitter.progress) } {
        if let Ok(report) = ReplayProgress::decode_from(&payload)
            && i64::try_from(report.clock_nanos).ok() == Some(scheduled)
        {
            emitter.early.fetch_add(1, Ordering::SeqCst);
        }
    }
    unsafe { send_payload(emitter.publisher, &scheduled.to_le_bytes()) };
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn add_emitting_cycle(bus: &mut Bus, emitter: &mut Emitter) {
    let label = c_text(&unique("emit_cycle"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                bus.waitset,
                label.as_ptr(),
                label.as_bytes().len(),
                PERIOD_NANOS,
                Some(emit_step),
                (&raw mut *emitter).cast::<c_void>(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    bus.cycles.push(cycle);
}

#[test]
fn progress_is_published_after_the_outputs_of_its_step() {
    let mut bus = Bus::new();
    let topic = format!("/{}", unique("ordering_out"));
    let (output, reader) = unsafe { create_pubsub(bus.node, &topic, 16) };
    let probe =
        unsafe { open_subscriber(bus.node, "/clock_progress", ReplayProgress::SCHEMA_HASH, 8) };
    let mut emitter = Emitter { publisher: output, progress: probe, early: AtomicUsize::new(0) };
    add_emitting_cycle(&mut bus, &mut emitter);
    bus.clock(BASE);
    bus.run();
    bus.progress();

    // Each sample advances the clock by one period, so each one runs exactly one step.
    for index in 1..=200_u64 {
        let nanos = BASE + index * PERIOD_NANOS;
        bus.clock(nanos);
        let mut acknowledged = false;
        for _ in 0..2_000 {
            run_for(bus.waitset, MS);
            if bus.progress().iter().any(|report| report.clock_nanos == nanos) {
                acknowledged = true;
                break;
            }
        }
        assert!(acknowledged, "the progress for {nanos} ns never arrived");
        let received = unsafe { receive_payload(reader) }.expect("the output is already queued");
        assert_eq!(received, nanos.to_le_bytes().to_vec(), "sample {index}");
    }
    assert_eq!(
        emitter.early.load(Ordering::SeqCst),
        0,
        "a step's progress was queued before its output was sent"
    );
    unsafe {
        hypha_subscriber_destroy(probe);
        hypha_subscriber_destroy(reader);
        hypha_publisher_destroy(output);
    }
}

fn report_payload(name: &str, clock_nanos: u64) -> Vec<u8> {
    use hypha_core::message::EncodeInto as _;
    let report =
        ReplayProgress { node: name.to_owned(), clock_nanos, clock_sent_nanos: clock_nanos };
    let mut payload = vec![0_u8; report.encoded_len()];
    let written = report.encode_into(&mut payload).expect("a report encodes");
    payload.truncate(written);
    payload
}

/// Every report `input` holds now, and its missing-sequence count after the drain.
fn drain_reports(input: *mut HyphaSubscriber) -> (usize, u64) {
    let mut received = 0;
    while unsafe { receive_payload(input) }.is_some() {
        received += 1;
    }
    let mut missing = 0_u64;
    assert_eq!(
        unsafe { hypha_subscriber_missing_sequences(input, &raw mut missing) },
        HyphaStatus::Ok
    );
    (received, missing)
}

unsafe fn progress_publisher(node: *mut HyphaNode) -> *mut HyphaPublisher {
    unsafe {
        open_publisher(
            node,
            "/clock_progress",
            "hypha_msgs/ReplayProgress",
            ReplayProgress::SCHEMA_HASH,
            "",
        )
    }
}

#[test]
fn a_progress_input_keeps_one_report_from_each_publisher() {
    // iceoryx2 gives each publisher connection its own queue, so a depth bounds the
    // unread reports of one publisher, not of all of them.
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = crate::common::create_node(&unique("progress_premise"));
    let input = unsafe { open_subscriber(node, "/clock_progress", ReplayProgress::SCHEMA_HASH, 1) };
    let publishers: Vec<*mut HyphaPublisher> =
        (0..8).map(|_| unsafe { progress_publisher(node) }).collect();
    for (index, publisher) in publishers.iter().enumerate() {
        let name = format!("peer_{index}");
        unsafe { send_payload(*publisher, &report_payload(&name, 1_000)) };
    }
    assert_eq!(drain_reports(input), (8, 0), "eight publishers, one report each, depth 1");
    unsafe {
        for publisher in publishers {
            hypha_publisher_destroy(publisher);
        }
        hypha_subscriber_destroy(input);
        hypha_node_destroy(node);
    }
}

#[test]
fn a_publisher_burst_beyond_the_input_depth_counts_missing_reports() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = crate::common::create_node(&unique("progress_burst"));
    let depth = 2;
    let input =
        unsafe { open_subscriber(node, "/clock_progress", ReplayProgress::SCHEMA_HASH, depth) };
    let publisher = unsafe { progress_publisher(node) };
    // A gap shows against the last report read, so one is read first.
    unsafe { send_payload(publisher, &report_payload("burst", 0)) };
    assert_eq!(drain_reports(input), (1, 0));
    for clock in 1..=depth + 1 {
        let nanos = u64::try_from(clock).expect("a small clock");
        unsafe { send_payload(publisher, &report_payload("burst", nanos)) };
    }
    assert_eq!(drain_reports(input), (depth, 1), "the oldest report is overwritten and counted");
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_subscriber_destroy(input);
        hypha_node_destroy(node);
    }
}
