//! The run reason of a bounded run.
#![allow(clippy::expect_used)]

mod common;
use crate::common::attach;
use crate::common::create_node;
use crate::common::unique;

use std::ffi::c_void;
use std::ptr;
use std::time::{Duration, Instant};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    waitset
}

fn empty_report() -> HyphaWaitSetRunReport {
    HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 }
}

extern "C" fn ignore_signal(_signal: libc::c_int) {}

/// Install `ignore_signal` for `SIGUSR1` without `SA_RESTART`, so the wait
/// returns with `EINTR` as it does for a real handled signal.
fn install_handled_signal() {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = ignore_signal as *const () as usize;
    action.sa_flags = 0;
    unsafe { libc::sigemptyset(&raw mut action.sa_mask) };
    let status = unsafe { libc::sigaction(libc::SIGUSR1, &raw const action, ptr::null_mut()) };
    assert_eq!(status, 0, "install the signal handler");
}

extern "C" fn stop_at_first_tick(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_STOP, status: 0 }
}

#[test]
fn a_handled_signal_does_not_end_a_bounded_run_early() {
    install_handled_signal();
    let node = create_node(&unique("run_reason_signal"));
    let waitset = create_waitset(node);
    let target = unsafe { libc::pthread_self() };
    let sender = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        unsafe { libc::pthread_kill(target, libc::SIGUSR1) };
    });

    let limit = Duration::from_millis(300);
    let mut report = empty_report();
    let started = Instant::now();
    let status = unsafe { hypha_waitset_run(waitset, 300_000_000, &raw mut report) };
    let elapsed = started.elapsed();
    sender.join().expect("signal sender");

    assert_eq!(status, HyphaStatus::Ok);
    assert!(elapsed + Duration::from_millis(20) >= limit, "the run ended after {elapsed:?}");
    assert_eq!(report.reason, HYPHA_RUN_REASON_DEADLINE);
    assert_eq!(report.status, HyphaStatus::Ok as i32);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_bounded_run_that_reaches_its_deadline_reports_deadline() {
    let node = create_node(&unique("run_reason_deadline"));
    let waitset = create_waitset(node);
    let mut report = empty_report();
    let status = unsafe { hypha_waitset_run(waitset, 30_000_000, &raw mut report) };
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(report.reason, HYPHA_RUN_REASON_DEADLINE);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_stop_asked_by_a_callback_reports_stopped_within_a_bounded_run() {
    let node = create_node(&unique("run_reason_stop"));
    let waitset = create_waitset(node);
    let mut attachment = 0;
    let status = unsafe {
        hypha_waitset_attach(
            waitset,
            &attach::timer(10_000_000, Some(stop_at_first_tick), ptr::null_mut(), None),
            &raw mut attachment,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    let mut report = empty_report();
    let started = Instant::now();
    let status = unsafe { hypha_waitset_run(waitset, 5_000_000_000, &raw mut report) };
    assert_eq!(status, HyphaStatus::Ok);
    assert!(started.elapsed() < Duration::from_secs(4), "the stop did not end the run");
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED);
    unsafe { hypha_node_destroy(node) };
}

/// The waitset a callback stops, set before the run starts.
static STOPPED_WAITSET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Set once `overrun_then_request_stop` has run.
static OVERRUN_RAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Busy-waits past the run's limit, then asks the run to stop.
extern "C" fn overrun_then_request_stop(
    _missed: u64,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(3) {
        std::hint::spin_loop();
    }
    let waitset = STOPPED_WAITSET.load(std::sync::atomic::Ordering::SeqCst) as *mut HyphaWaitSet;
    let _ = unsafe { hypha_waitset_request_stop(waitset) };
    OVERRUN_RAN.store(true, std::sync::atomic::Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_stop_requested_by_a_callback_that_overruns_the_limit_reports_stopped() {
    let node = create_node(&unique("run_reason_overrun"));
    let waitset = create_waitset(node);
    STOPPED_WAITSET.store(waitset as usize, std::sync::atomic::Ordering::SeqCst);
    let mut attachment = 0;
    let status = unsafe {
        hypha_waitset_attach(
            waitset,
            &attach::timer(500_000, Some(overrun_then_request_stop), ptr::null_mut(), None),
            &raw mut attachment,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    // A loaded host can end a 1 ms run before the timer is first due; the next one is tried.
    let started = Instant::now();
    loop {
        let mut report = empty_report();
        let status = unsafe { hypha_waitset_run(waitset, 1_000_000, &raw mut report) };
        assert_eq!(status, HyphaStatus::Ok);
        if OVERRUN_RAN.load(std::sync::atomic::Ordering::SeqCst) {
            assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED, "the stop request was lost");
            break;
        }
        assert_eq!(report.reason, HYPHA_RUN_REASON_DEADLINE);
        assert!(started.elapsed() < Duration::from_secs(10), "the timer never ran");
    }
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_timer_callback_that_overruns_the_limit_and_stops_reports_stopped() {
    let node = create_node(&unique("run_reason_timer"));
    let waitset = create_waitset(node);
    STOPPED_WAITSET.store(waitset as usize, std::sync::atomic::Ordering::SeqCst);
    let mut attachment = 0;
    let status = unsafe {
        hypha_waitset_attach(
            waitset,
            &attach::timer(500_000, Some(overrun_timer_then_request_stop), ptr::null_mut(), None),
            &raw mut attachment,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    // A loaded host can end a 1 ms run before the timer is first due; the next one is tried.
    let started = Instant::now();
    loop {
        let mut report = empty_report();
        let status = unsafe { hypha_waitset_run(waitset, 1_000_000, &raw mut report) };
        assert_eq!(status, HyphaStatus::Ok);
        if OVERRUN_RAN.load(std::sync::atomic::Ordering::SeqCst) {
            assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED, "the stop request was lost");
            break;
        }
        assert_eq!(report.reason, HYPHA_RUN_REASON_DEADLINE);
        assert!(started.elapsed() < Duration::from_secs(10), "the timer never ran");
    }
    unsafe { hypha_node_destroy(node) };
}

extern "C" fn overrun_timer_then_request_stop(
    _missed: u64,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    overrun_then_request_stop(0, ptr::null_mut())
}
