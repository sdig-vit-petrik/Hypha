//! `missed` of a timer callback is a per-call delta: the periods skipped since the last
//! call, not since the first. The clock is simulated, so the counts are exact.
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

/// Run one turn: a run always takes its first turn, which serves the due timers.
fn one_turn(waitset: *mut HyphaWaitSet) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, 1_000_000, &raw mut report) }, HyphaStatus::Ok);
}

#[test]
fn a_timer_reports_missed_as_a_per_call_delta() {
    const START: i64 = 1_000_000_000;
    let _clock = common::replay::SimulatedClock::start(START);
    let node = common::create_node(&unique("missed_delta_sim"));
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let recorder =
        Recorder { missed: Mutex::new(Vec::new()), stall_at: usize::MAX, stall: Duration::ZERO };
    let context = ptr::from_ref(&recorder).cast_mut().cast::<c_void>();
    let record = attach::timer(PERIOD_NANOS, Some(record_missed), context, None);
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    one_turn(waitset);
    // Two periods on time, a jump of 55 ms, and one more period.
    for offset_ms in [10, 20, 75, 80] {
        common::replay::set_clock(START + offset_ms * 1_000_000);
        one_turn(waitset);
    }
    unsafe { hypha_node_destroy(node) };
    let missed = recorder.missed.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    // On time at 10 and 20 ms; the call at 75 ms skipped the deadlines at 30 to 60 ms;
    // the next is on time again: a delta, not a running total.
    assert_eq!(missed, [0, 0, 4, 0]);
}

#[test]
fn a_host_timer_does_not_count_the_time_before_its_first_run_as_missed() {
    // 60 ms before the run, six periods: the first call misses none of them. Later calls
    // are left out: a loaded host may make them late.
    let missed = missed_per_call(usize::MAX, Duration::from_millis(60));
    assert_eq!(missed.first().copied(), Some(0), "{missed:?}");
}
