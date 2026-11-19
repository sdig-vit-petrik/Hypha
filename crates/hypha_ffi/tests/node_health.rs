//! A node reports its own execution health.
#![allow(clippy::expect_used)]

mod common;
use crate::common::attach;
use crate::common::create_unique_node;

use std::os::raw::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

const MILLISECOND: u64 = 1_000_000;

unsafe fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    let status = unsafe { hypha_node_default_waitset(node, &raw mut waitset) };
    assert_eq!(status, HyphaStatus::Ok);
    waitset
}

unsafe fn attach_timer(waitset: *mut HyphaWaitSet, callback: HyphaIntervalCallback) {
    let mut attachment = 0;
    let status = unsafe {
        hypha_waitset_attach(
            waitset,
            &attach::timer(5 * MILLISECOND, callback, ptr::null_mut(), None),
            &raw mut attachment,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
}

unsafe fn run_for(waitset: *mut HyphaWaitSet, limit_nanos: u64) -> HyphaWaitSetRunReport {
    let mut report =
        HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
    let status = unsafe { hypha_waitset_run(waitset, limit_nanos, &raw mut report) };
    assert_eq!(status, HyphaStatus::Ok);
    report
}

#[derive(Debug)]
struct Report {
    callbacks_invoked: u64,
    callback_failures: u64,
    slow_callbacks: u64,
    last_sweep_end_nanos: u64,
    last_callback_status: i32,
    is_sim_time: u64,
}

unsafe fn health(node: *const HyphaNode) -> Report {
    let value = |name: &str| common::node_metric(node, name).expect("a node metric");
    Report {
        callbacks_invoked: value("node.callbacks_invoked"),
        callback_failures: value("node.callback_failures"),
        slow_callbacks: value("node.slow_callbacks"),
        last_sweep_end_nanos: value("node.last_sweep_end_nanos"),
        last_callback_status: i32::try_from(value("node.last_callback_status"))
            .expect("a callback status fits in i32"),
        is_sim_time: value("node.is_sim_time"),
    }
}

const CONTINUE: HyphaCallbackResult =
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 };

/// Counts the calls of the callbacks below. Each test runs in its own process.
static CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn succeed(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    CALLS.fetch_add(1, Ordering::SeqCst);
    CONTINUE
}

/// Reports a failure on its first call and keeps the run going.
extern "C" fn fail_once_then_succeed(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    let seen = CALLS.fetch_add(1, Ordering::SeqCst);
    if seen == 0 {
        return HyphaCallbackResult {
            action: HYPHA_ACTION_CONTINUE,
            status: HyphaStatus::Panic as i32,
        };
    }
    CONTINUE
}

extern "C" fn fail_without_a_status(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    CALLS.fetch_add(1, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_FAIL, status: 0 }
}

extern "C" fn stop_after_one_failure(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    CALLS.fetch_add(1, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_STOP, status: HyphaStatus::Timeout as i32 }
}

extern "C" fn run_slowly(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    CALLS.fetch_add(1, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(15));
    CONTINUE
}

#[test]
fn a_new_node_reports_nothing_yet() {
    let node = create_unique_node("new");

    let report = unsafe { health(node) };

    assert_eq!(report.callbacks_invoked, 0);
    assert_eq!(report.callback_failures, 0);
    assert_eq!(report.slow_callbacks, 0);
    assert_eq!(report.last_sweep_end_nanos, 0, "no sweep has ended yet");
    assert_eq!(report.last_callback_status, 0);
    assert_eq!(report.is_sim_time, 0);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_clean_run_counts_its_calls_and_the_end_of_a_sweep() {
    let node = create_unique_node("clean");
    let waitset = unsafe { create_waitset(node) };
    unsafe { attach_timer(waitset, Some(succeed)) };

    let run = unsafe { run_for(waitset, 60 * MILLISECOND) };

    assert_eq!(run.reason, HYPHA_RUN_REASON_DEADLINE);
    let report = unsafe { health(node) };
    let calls = CALLS.load(Ordering::SeqCst);
    assert!(calls > 0, "the timer must have run");
    assert_eq!(report.callbacks_invoked, calls);
    assert_eq!(report.callback_failures, 0);
    assert_eq!(report.last_callback_status, 0);
    assert!(report.last_sweep_end_nanos > 0, "a sweep ended during the run");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_failure_the_run_survives_is_counted_with_its_status() {
    let node = create_unique_node("survived");
    let waitset = unsafe { create_waitset(node) };
    unsafe { attach_timer(waitset, Some(fail_once_then_succeed)) };

    let run = unsafe { run_for(waitset, 60 * MILLISECOND) };

    assert_eq!(run.reason, HYPHA_RUN_REASON_DEADLINE, "a continue result must not end the run");
    assert_eq!(run.status, HyphaStatus::Ok as i32);
    let report = unsafe { health(node) };
    assert!(CALLS.load(Ordering::SeqCst) >= 2, "the callback must run again after its failure");
    assert_eq!(report.callbacks_invoked, CALLS.load(Ordering::SeqCst));
    assert_eq!(report.callback_failures, 1);
    assert_eq!(report.last_callback_status, HyphaStatus::Panic as i32);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_fail_action_without_a_status_counts_as_a_backend_failure() {
    let node = create_unique_node("fail");
    let waitset = unsafe { create_waitset(node) };
    unsafe { attach_timer(waitset, Some(fail_without_a_status)) };

    let run = unsafe { run_for(waitset, 500 * MILLISECOND) };

    assert_eq!(run.reason, HYPHA_RUN_REASON_RUNTIME_FAILED);
    let report = unsafe { health(node) };
    assert_eq!(report.callbacks_invoked, 1);
    assert_eq!(report.callback_failures, 1);
    assert_eq!(report.last_callback_status, HyphaStatus::Backend as i32);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_nonzero_status_with_the_stop_action_is_a_failure() {
    let node = create_unique_node("stop");
    let waitset = unsafe { create_waitset(node) };
    unsafe { attach_timer(waitset, Some(stop_after_one_failure)) };

    let run = unsafe { run_for(waitset, 500 * MILLISECOND) };

    assert_eq!(run.reason, HYPHA_RUN_REASON_STOPPED);
    let report = unsafe { health(node) };
    assert_eq!(report.callback_failures, 1);
    assert_eq!(report.last_callback_status, HyphaStatus::Timeout as i32);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn every_run_of_a_node_adds_to_one_report() {
    let node = create_unique_node("shared");
    let waitset = unsafe { create_waitset(node) };
    unsafe { attach_timer(waitset, Some(fail_without_a_status)) };

    let first_run = unsafe { run_for(waitset, 500 * MILLISECOND) };
    let second_run = unsafe { run_for(waitset, 500 * MILLISECOND) };

    assert_eq!(first_run.reason, HYPHA_RUN_REASON_RUNTIME_FAILED);
    assert_eq!(second_run.reason, HYPHA_RUN_REASON_RUNTIME_FAILED);

    let report = unsafe { health(node) };
    assert_eq!(report.callbacks_invoked, 2);
    assert_eq!(report.callback_failures, 2, "the counters are cumulative for the node");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_slow_callback_is_counted_in_the_run_report_and_the_node_report() {
    let node = create_unique_node("slow");
    let waitset = unsafe { create_waitset(node) };
    unsafe { attach_timer(waitset, Some(run_slowly)) };

    let run = unsafe { run_for(waitset, 60 * MILLISECOND) };

    assert!(run.slow_callbacks > 0, "a 15 ms callback is over the 10 ms threshold");
    let report = unsafe { health(node) };
    assert_eq!(report.slow_callbacks, run.slow_callbacks);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn the_sweep_end_is_framework_time_and_follows_simulated_time() {
    const SIMULATED_NANOS: i64 = 5_000_000_000;
    let node = create_unique_node("simulated");
    let waitset = unsafe { create_waitset(node) };
    unsafe { attach_timer(waitset, Some(succeed)) };
    assert_eq!(hypha_clock_enable_sim_time(SIMULATED_NANOS), HyphaStatus::Ok as i32);

    let run = unsafe { run_for(waitset, 20 * MILLISECOND) };
    let report = unsafe { health(node) };
    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);

    assert_eq!(run.reason, HYPHA_RUN_REASON_DEADLINE);
    assert_eq!(report.is_sim_time, 1);
    assert_eq!(report.last_sweep_end_nanos, 5_000_000_000);
    unsafe { hypha_node_destroy(node) };
}
