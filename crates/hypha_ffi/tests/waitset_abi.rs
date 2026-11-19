//! C ABI tests for caller-owned execution.
#![allow(clippy::expect_used)]

mod common;
use crate::common::attach;
use crate::common::create_node;
use crate::common::unique;

use std::os::raw::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

unsafe fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    let status = unsafe { hypha_node_default_waitset(node, &raw mut waitset) };
    assert_eq!(status, HyphaStatus::Ok);
    waitset
}

fn empty_report() -> HyphaWaitSetRunReport {
    HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 }
}

/// Counts timer callbacks, then stops the run.
static TICKS: AtomicU64 = AtomicU64::new(0);

extern "C" fn count_then_stop(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    let seen = TICKS.fetch_add(1, Ordering::SeqCst);
    if seen >= 2 {
        return HyphaCallbackResult { action: HYPHA_ACTION_STOP, status: 0 };
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn fail_at_once(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_FAIL, status: HyphaStatus::Timeout as i32 }
}

/// Records that a context release ran, and how often.
static RELEASES: AtomicU64 = AtomicU64::new(0);

extern "C" fn record_release(_context: *mut c_void) {
    RELEASES.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn a_timer_registration_runs_and_a_callback_can_stop_the_run() {
    let node = create_node(&unique("interval"));
    let waitset = unsafe { create_waitset(node) };
    TICKS.store(0, Ordering::SeqCst);

    let mut attachment = 0;
    let status = unsafe {
        hypha_waitset_attach(
            waitset,
            &attach::timer(10_000_000, Some(count_then_stop), ptr::null_mut(), None),
            &raw mut attachment,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    assert_ne!(attachment, 0, "a registration id is never zero");

    let mut report = empty_report();
    let run = unsafe { hypha_waitset_run(waitset, 0, &raw mut report) };

    assert_eq!(run, HyphaStatus::Ok, "a completed run returns Ok and fills the report");
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED);
    assert_eq!(report.status, HyphaStatus::Ok as i32);
    assert!(TICKS.load(Ordering::SeqCst) >= 3);

    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_failing_callback_reports_its_own_status_and_attachment() {
    let node = create_node(&unique("failing"));
    let waitset = unsafe { create_waitset(node) };

    let mut attachment = 0;
    assert_eq!(
        unsafe {
            hypha_waitset_attach(
                waitset,
                &attach::timer(10_000_000, Some(fail_at_once), ptr::null_mut(), None),
                &raw mut attachment,
            )
        },
        HyphaStatus::Ok
    );

    let mut report = empty_report();
    let run = unsafe { hypha_waitset_run(waitset, 0, &raw mut report) };

    assert_eq!(run, HyphaStatus::Ok, "an execution failure is not an API failure");
    assert_eq!(report.reason, HYPHA_RUN_REASON_RUNTIME_FAILED);
    assert_eq!(report.status, HyphaStatus::Timeout as i32, "the callback chose this status");
    assert_eq!(report.attachment, attachment);

    unsafe { hypha_node_destroy(node) };
}

#[test]
fn detach_releases_its_context_exactly_once() {
    let node = create_node(&unique("detach"));
    let waitset = unsafe { create_waitset(node) };
    RELEASES.store(0, Ordering::SeqCst);

    let mut attachment = 0;
    assert_eq!(
        unsafe {
            hypha_waitset_attach(
                waitset,
                &attach::timer(
                    10_000_000,
                    Some(count_then_stop),
                    ptr::null_mut(),
                    Some(record_release),
                ),
                &raw mut attachment,
            )
        },
        HyphaStatus::Ok
    );

    assert_eq!(unsafe { hypha_waitset_detach(waitset, attachment) }, HyphaStatus::Ok);
    assert_eq!(RELEASES.load(Ordering::SeqCst), 1);

    assert_eq!(
        unsafe { hypha_waitset_detach(waitset, attachment) },
        HyphaStatus::InvalidArgument,
        "an unknown id is refused"
    );

    // Destroying the node must not run the release a second time.
    unsafe { hypha_node_destroy(node) };
    assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
}

#[test]
fn a_stop_requested_before_a_run_starts_is_not_lost() {
    let node = create_node(&unique("stop_state"));
    let waitset = unsafe { create_waitset(node) };
    TICKS.store(0, Ordering::SeqCst);

    let mut attachment = 0;
    assert_eq!(
        unsafe {
            hypha_waitset_attach(
                waitset,
                &attach::timer(10_000_000, Some(count_then_stop), ptr::null_mut(), None),
                &raw mut attachment,
            )
        },
        HyphaStatus::Ok
    );

    // The status reports that no run was in progress. The request is still
    // recorded, so a stop from another thread cannot lose the race.
    assert_eq!(unsafe { hypha_waitset_request_stop(waitset) }, HyphaStatus::InvalidState);

    let mut report = empty_report();
    let run = unsafe { hypha_waitset_run(waitset, 0, &raw mut report) };

    assert_eq!(run, HyphaStatus::Ok);
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED);
    assert_eq!(TICKS.load(Ordering::SeqCst), 0, "the pending stop ended the run at once");

    // The run consumed the request, so the next run is not stopped by it.
    TICKS.store(0, Ordering::SeqCst);
    let mut second = empty_report();
    assert_eq!(unsafe { hypha_waitset_run(waitset, 0, &raw mut second) }, HyphaStatus::Ok);
    assert!(TICKS.load(Ordering::SeqCst) >= 3, "the next run is not stopped by the old request");

    unsafe { hypha_node_destroy(node) };
}
