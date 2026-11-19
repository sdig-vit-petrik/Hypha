//! `missed` of a host timer callback is a per-call delta.
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;

use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;
use std::time::Duration;

use hypha_ffi::waitset::{HYPHA_ACTION_CONTINUE, HyphaCallbackResult};
use hypha_ffi::*;

use crate::common::{attach, unique};

const PERIOD_NANOS: u64 = 10_000_000;

/// The `missed` of each call. The call given by `stall_at` blocks for `stall`.
struct Recorder {
    missed: Mutex<Vec<u64>>,
    stall_at: usize,
    stall: Duration,
}

extern "C" fn record_missed(missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    let recorder = unsafe { &*context.cast::<Recorder>() };
    let calls = {
        let mut seen = recorder.missed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        seen.push(missed);
        seen.len()
    };
    if calls == recorder.stall_at {
        std::thread::sleep(recorder.stall);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// Attach a host timer to a fresh node's `WaitSet`, wait `before_run`, run 200 ms, and
/// return the `missed` of each call.
fn missed_per_call(stall_at: usize, before_run: Duration) -> Vec<u64> {
    let node = common::create_node(&unique("missed_delta"));
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let recorder =
        Recorder { missed: Mutex::new(Vec::new()), stall_at, stall: Duration::from_millis(55) };
    let context = ptr::from_ref(&recorder).cast_mut().cast::<c_void>();
    let record = attach::timer(PERIOD_NANOS, Some(record_missed), context, None);
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    std::thread::sleep(before_run);
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(
        unsafe { hypha_waitset_run(waitset, 200_000_000, &raw mut report) },
        HyphaStatus::Ok
    );
    unsafe { hypha_node_destroy(node) };
    recorder.missed.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn assert_one_delta(missed: &[u64]) {
    let stalled: Vec<u64> = missed.iter().copied().filter(|count| *count > 0).collect();
    assert_eq!(stalled.len(), 1, "one delta after the stall: {missed:?}");
    let delta = stalled.first().copied().unwrap_or(0);
    assert!((4..=6).contains(&delta), "a 55 ms stall of a 10 ms timer misses 4 to 6: {missed:?}");
}

#[test]
fn a_host_timer_reports_missed_as_a_per_call_delta() {
    let missed = missed_per_call(2, Duration::ZERO);
    assert_one_delta(&missed);
}

#[test]
fn a_host_timer_does_not_count_the_time_before_its_first_run_as_missed() {
    let missed = missed_per_call(usize::MAX, Duration::from_millis(60));
    assert_eq!(missed.first().copied(), Some(0), "{missed:?}");
    assert!(missed.iter().all(|count| *count == 0), "{missed:?}");
}
