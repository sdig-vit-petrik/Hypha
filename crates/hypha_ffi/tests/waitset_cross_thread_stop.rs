//! A running `WaitSet` is stopped from another thread while a cycle steps and a
//! subscription callback delivers samples.
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;
use common::attach;
use common::replay::{create_pubsub, create_waitset, send_payload};
use common::{create_node, unique};

use std::ffi::{CString, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

static DELIVERIES: AtomicU64 = AtomicU64::new(0);
static STEPS: AtomicU64 = AtomicU64::new(0);

extern "C" fn count_delivery(
    _delivery: *const HyphaDelivery,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    DELIVERIES.fetch_add(1, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn count_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    STEPS.fetch_add(1, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// A committed 1 kHz cycle on `waitset`, with no input.
unsafe fn committed_cycle(waitset: *mut HyphaWaitSet) -> *mut HyphaCycle {
    let name = CString::new("stopped_cycle").expect("cycle name");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                1_000_000,
                Some(count_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    cycle
}

/// Attach `subscriber` to `waitset`, delivering on each event.
unsafe fn attach_subscription(
    waitset: *mut HyphaWaitSet,
    subscriber: *mut HyphaSubscriber,
) -> HyphaAttachmentId {
    let record = attach::subscription(subscriber, Some(count_delivery), ptr::null_mut(), None);
    let mut id = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut id) },
        HyphaStatus::Ok
    );
    id
}

/// Publish from `publisher` until `counter` reaches `at_least`, within a bound.
unsafe fn publish_until(publisher: *mut HyphaPublisher, counter: &AtomicU64, at_least: u64) {
    let started = Instant::now();
    while counter.load(Ordering::SeqCst) < at_least {
        assert!(started.elapsed() < Duration::from_secs(10), "the run made no progress");
        unsafe { send_payload(publisher, &[1]) };
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn another_thread_stops_a_run_that_steps_a_cycle_and_delivers_samples() {
    let node = create_node(&unique("cross_stop"));
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) =
        unsafe { create_pubsub(node, &format!("/{}", unique("cross_stop")), 8) };
    let attachment = unsafe { attach_subscription(waitset, subscriber) };
    let cycle = unsafe { committed_cycle(waitset) };
    DELIVERIES.store(0, Ordering::SeqCst);
    STEPS.store(0, Ordering::SeqCst);

    let handle = waitset as usize;
    let runner = std::thread::spawn(move || {
        let waitset = handle as *mut HyphaWaitSet;
        let mut report =
            HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
        let status = unsafe { hypha_waitset_run(waitset, 0, &raw mut report) };
        (status, report)
    });

    // Both kinds of work ran, so the run is in progress and inside its dispatch.
    unsafe { publish_until(publisher, &DELIVERIES, 3) };
    unsafe { publish_until(publisher, &STEPS, 3) };

    // Every other call on a running `WaitSet` is refused, from this thread too.
    assert_eq!(unsafe { hypha_waitset_detach(waitset, attachment) }, HyphaStatus::InvalidState);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::InvalidState);
    let mut late = 0;
    let late_record = attach::timer(1_000_000, None, ptr::null_mut(), None);
    assert_ne!(
        unsafe { hypha_waitset_attach(waitset, &raw const late_record, &raw mut late) },
        HyphaStatus::Ok
    );

    assert_eq!(unsafe { hypha_waitset_request_stop(waitset) }, HyphaStatus::Ok);
    let (status, report) = runner.join().expect("the run thread finished");
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED);
    assert_eq!(report.status, HyphaStatus::Ok as i32);
    assert!(DELIVERIES.load(Ordering::SeqCst) >= 3);
    assert!(STEPS.load(Ordering::SeqCst) >= 3);

    // Once stopped, the same calls succeed.
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_waitset_detach(waitset, attachment) }, HyphaStatus::Ok);
    unsafe { hypha_subscriber_destroy(subscriber) };
    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_subscriber_released_by_its_creator_is_still_dispatched_until_detach() {
    let node = create_node(&unique("shared_sub"));
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) =
        unsafe { create_pubsub(node, &format!("/{}", unique("shared_sub")), 8) };
    let attachment = unsafe { attach_subscription(waitset, subscriber) };
    // The registration shares the subscriber, so the caller's handle can go first.
    unsafe { hypha_subscriber_destroy(subscriber) };
    DELIVERIES.store(0, Ordering::SeqCst);

    let handle = waitset as usize;
    let runner = std::thread::spawn(move || {
        let waitset = handle as *mut HyphaWaitSet;
        let mut report =
            HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
        assert_eq!(unsafe { hypha_waitset_run(waitset, 0, &raw mut report) }, HyphaStatus::Ok);
        report
    });
    unsafe { publish_until(publisher, &DELIVERIES, 3) };
    assert_eq!(unsafe { hypha_waitset_request_stop(waitset) }, HyphaStatus::Ok);
    let report = runner.join().expect("the run thread finished");
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED);

    // The detach drops the last owner of the subscriber.
    assert_eq!(unsafe { hypha_waitset_detach(waitset, attachment) }, HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_node_destroy(node) };
}

static TICKS: AtomicU64 = AtomicU64::new(0);

extern "C" fn count_tick(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    TICKS.fetch_add(1, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn wait_for(counter: &AtomicU64, at_least: u64) {
    let started = Instant::now();
    while counter.load(Ordering::SeqCst) < at_least {
        assert!(started.elapsed() < Duration::from_secs(10), "the run made no progress");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn run_on_another_thread(
    waitset: *mut HyphaWaitSet,
) -> std::thread::JoinHandle<(HyphaStatus, HyphaWaitSetRunReport)> {
    let handle = waitset as usize;
    std::thread::spawn(move || {
        let waitset = handle as *mut HyphaWaitSet;
        let mut report =
            HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
        // SAFETY: the test keeps the node, and its `WaitSet`, alive until this thread joined.
        let status = unsafe { hypha_waitset_run(waitset, 0, &raw mut report) };
        (status, report)
    })
}

#[test]
fn another_thread_fails_a_running_run_with_application_failed() {
    let node = create_node(&unique("cross_failure"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { create_waitset(node) };
    let record = attach::timer(1_000_000, Some(count_tick), ptr::null_mut(), None);
    let mut id = 0;
    // SAFETY: `waitset` is live; `record` and `id` are locals.
    let attached = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut id) };
    assert_eq!(attached, HyphaStatus::Ok);
    TICKS.store(0, Ordering::SeqCst);

    let runner = run_on_another_thread(waitset);
    // The timer ran, so the run is in progress.
    wait_for(&TICKS, 2);
    // SAFETY: `waitset` is live; this is a call another thread may make during a run.
    let requested = unsafe { hypha_waitset_request_failure(waitset) };
    assert_eq!(requested, HyphaStatus::Ok, "a run was in progress");
    let (status, report) = runner.join().expect("the run thread finished");
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(report.reason, HYPHA_RUN_REASON_RUNTIME_FAILED);
    assert_eq!(report.status, HyphaStatus::ApplicationFailed as i32);
    assert_eq!(report.attachment, 0, "the failure belongs to no registration");

    // SAFETY: `node` is live and destroyed once, after the run thread joined.
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn another_thread_reads_the_health_of_a_cycle_while_its_waitset_runs() {
    let node = create_node(&unique("cross_health"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { create_waitset(node) };
    // SAFETY: as above.
    let cycle = unsafe { committed_cycle(waitset) };

    let runner = run_on_another_thread(waitset);
    let started = Instant::now();
    loop {
        let mut health = std::mem::MaybeUninit::<HyphaCycleHealth>::zeroed();
        // SAFETY: `cycle` is live until its destroy below; `health` is a local.
        let read = unsafe { hypha_cycle_health(cycle, health.as_mut_ptr()) };
        assert_eq!(read, HyphaStatus::Ok);
        // SAFETY: the call wrote the health on `Ok`, and zeroes are a valid health anyway.
        let steps = unsafe { health.assume_init() }.steps;
        if steps >= 3 {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "the cycle made no step");
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut binding = std::mem::MaybeUninit::<HyphaCycleBindingView>::zeroed();
    // SAFETY: as above; the cycle has no input, so binding 0 is unknown.
    let unknown = unsafe { hypha_cycle_binding(cycle, 0, binding.as_mut_ptr()) };
    assert_eq!(unknown, HyphaStatus::InvalidArgument);

    // SAFETY: `waitset` is live.
    assert_eq!(unsafe { hypha_waitset_request_stop(waitset) }, HyphaStatus::Ok);
    let (status, report) = runner.join().expect("the run thread finished");
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED);
    // SAFETY: the run returned; `cycle` is live and destroyed once.
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    // SAFETY: `node` is live and destroyed once, last.
    unsafe { hypha_node_destroy(node) };
}
