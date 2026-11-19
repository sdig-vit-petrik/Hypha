//! Replay progress against the threads of a node. The clock case holds [`SERIAL`].
#![allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]

use std::ffi::c_void;
use std::ptr;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hypha_core::message::DecodeOwned as _;
use hypha_msgs::hypha_msgs::ReplayProgress;

mod common;
use crate::common::replay::*;
use crate::common::{c_text, create_node, unique};
use hypha_ffi::waitset::*;
use hypha_ffi::*;

const MS: u64 = 1_000_000;
const SECOND: u64 = 1_000_000_000;
/// The first clock value of every case; the cycles anchor on it.
const BASE: u64 = SECOND;
/// The cycles run at 50 Hz, so the deadlines fall every 20 ms after the anchor.
const PERIOD_NANOS: u64 = 20 * MS;

/// Sets the flag when the reader ends, also by a panic.
struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[test]
fn a_reader_on_another_thread_finds_the_output_sent_before_the_progress() {
    const ITERATIONS: u64 = 3_000;
    /// How far the writer may run ahead of the reader. The queues hold 16 samples.
    const WINDOW: u64 = 8;
    let node = create_node(&unique("ordering_threads"));
    let output_topic = format!("/{}", unique("ordering_output"));
    let progress_topic = format!("/{}", unique("ordering_progress"));
    let (output, output_reader) = unsafe { create_pubsub(node, &output_topic, 16) };
    let (progress, progress_reader) = unsafe { create_pubsub(node, &progress_topic, 16) };
    // Raw pointers cross into the scoped threads as addresses, each used by one thread.
    let (output, progress) = (output as usize, progress as usize);
    let (output_reader, progress_reader) = (output_reader as usize, progress_reader as usize);
    let consumed = AtomicU64::new(0);
    // Set when the reader stops, so a writer waiting on it stops too.
    let reader_stopped = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for index in 0..ITERATIONS {
                while index >= consumed.load(Ordering::Acquire).saturating_add(WINDOW) {
                    if reader_stopped.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::yield_now();
                }
                unsafe {
                    send_payload(output as *mut HyphaPublisher, &index.to_le_bytes());
                    send_payload(progress as *mut HyphaPublisher, &index.to_le_bytes());
                }
            }
        });
        scope.spawn(|| {
            let _stop = StopOnDrop(&reader_stopped);
            for index in 0..ITERATIONS {
                let deadline = Instant::now() + Duration::from_secs(10);
                let report = loop {
                    let polled =
                        unsafe { receive_payload(progress_reader as *mut HyphaSubscriber) };
                    if let Some(payload) = polled {
                        break payload;
                    }
                    assert!(Instant::now() < deadline, "progress {index} never arrived");
                    std::thread::yield_now();
                };
                assert_eq!(report, index.to_le_bytes().to_vec(), "progress {index}");
                // The output was sent before the progress, so it is the next sample taken.
                let sent = unsafe { receive_payload(output_reader as *mut HyphaSubscriber) };
                assert_eq!(
                    sent.expect("the output sent before the progress is queued"),
                    index.to_le_bytes().to_vec(),
                    "output {index}"
                );
                consumed.store(index + 1, Ordering::Release);
            }
        });
    });
    unsafe {
        hypha_subscriber_destroy(progress_reader as *mut HyphaSubscriber);
        hypha_subscriber_destroy(output_reader as *mut HyphaSubscriber);
        hypha_publisher_destroy(progress as *mut HyphaPublisher);
        hypha_publisher_destroy(output as *mut HyphaPublisher);
        hypha_node_destroy(node);
    }
}

fn commit_recording_cycle(waitset: *mut HyphaWaitSet, recorder: &Recorder) -> *mut HyphaCycle {
    let label = c_text(&unique("threaded_cycle"));
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

/// Take every progress report queued so far; each must name a clock value whose deadline
/// the replay cycle has already stepped. Returns the newest reported clock value.
fn check_reports(progress: *mut HyphaSubscriber, recorder: &Recorder) -> Option<u64> {
    let mut newest = None;
    while let Some(payload) = unsafe { receive_payload(progress) } {
        let report = ReplayProgress::decode_from(&payload).expect("a progress report decodes");
        let deadline = report.clock_nanos / PERIOD_NANOS * PERIOD_NANOS;
        if deadline > BASE {
            let stepped = recorder
                .steps()
                .iter()
                .any(|step| u64::try_from(step.scheduled).is_ok_and(|at| at == deadline));
            assert!(
                stepped,
                "progress named clock {} before its deadline {deadline} was stepped",
                report.clock_nanos
            );
        }
        newest = Some(report.clock_nanos);
    }
    newest
}

#[test]
fn progress_never_names_a_clock_whose_deadline_is_not_stepped_while_the_waitset_runs_on_another_thread()
 {
    const SAMPLES: u64 = 300;
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_sim_node(&unique("threaded_progress"));
    let cycled = unsafe { create_waitset(node) };
    let recorder = Recorder::new(Duration::ZERO);
    let cycle = commit_recording_cycle(cycled, &recorder);
    let clock = unsafe { clock_publisher(node) };
    let progress =
        unsafe { open_subscriber(node, "/clock_progress", ReplayProgress::SCHEMA_HASH, 8) };
    let final_nanos = BASE + SAMPLES * PERIOD_NANOS;
    let stop = AtomicBool::new(false);
    // The raw pointer crosses into the scoped thread as an address, used by that thread only.
    let cycled_address = cycled as usize;
    std::thread::scope(|scope| {
        // Stops the WaitSet when this scope ends, also by a panic.
        let _stop_all = StopOnDrop(&stop);
        {
            let stop = &stop;
            scope.spawn(move || {
                let waitset = cycled_address as *mut HyphaWaitSet;
                while !stop.load(Ordering::Acquire) {
                    run_for(waitset, MS);
                }
            });
        }
        // The cycle anchors at the first clock value its WaitSet sees, so the anchor
        // must settle before the clock moves on: wait for its report.
        let deadline = Instant::now() + Duration::from_secs(10);
        unsafe { publish_clock(clock, BASE) };
        let mut anchored = None;
        while anchored != Some(BASE) {
            assert!(Instant::now() < deadline, "the anchoring clock value was never reported");
            anchored = check_reports(progress, &recorder).or(anchored);
            std::thread::yield_now();
        }
        for index in 1..=SAMPLES {
            unsafe { publish_clock(clock, BASE + index * PERIOD_NANOS) };
            check_reports(progress, &recorder);
        }
        // The last sample is reported once its deadline has been stepped.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut newest = None;
        while newest != Some(final_nanos) {
            assert!(Instant::now() < deadline, "the last clock sample was never reported");
            newest = check_reports(progress, &recorder).or(newest);
            std::thread::yield_now();
        }
    });
    unsafe {
        assert_eq!(hypha_cycle_destroy(cycle), HyphaStatus::Ok);
        hypha_subscriber_destroy(progress);
        hypha_publisher_destroy(clock);
        hypha_node_destroy(node);
    }
    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
}
