//! While a `WaitSet` runs on one thread, calls from another that would run, detach,
//! destroy or configure are refused with `InvalidState` and change nothing.
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;
use common::attach;
use common::replay::{create_pubsub, create_waitset, send_payload};
use common::supervision::ranges;
use common::{create_node, unique};

use std::ffi::{CString, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

static STEPS: AtomicU64 = AtomicU64::new(0);
static RELEASES: AtomicU32 = AtomicU32::new(0);

extern "C" fn count_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    STEPS.fetch_add(1, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn count_release(_context: *mut c_void) {
    RELEASES.fetch_add(1, Ordering::SeqCst);
}

fn last_error() -> String {
    let mut text = [0_u8; 256];
    let mut length = 0_usize;
    // SAFETY: `text` and `length` are locals of the sizes given.
    let read = unsafe { hypha_last_error(text.as_mut_ptr().cast(), text.len(), &raw mut length) };
    assert_eq!(read, HyphaStatus::Ok);
    String::from_utf8(text.get(..length).expect("the length fits").to_vec()).expect("UTF-8")
}

extern "C" fn no_tick(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// `hypha_cycle_create` of `name` at `period_nanos` with the counted release.
///
/// # Safety
/// `waitset` must be live.
unsafe fn create_cycle(
    waitset: *mut HyphaWaitSet,
    name: &str,
    period_nanos: u64,
) -> (HyphaStatus, *mut HyphaCycle) {
    let name = CString::new(name).expect("cycle name");
    // A sentinel: a refused create writes null here.
    let mut cycle = ptr::dangling_mut::<HyphaCycle>();
    // SAFETY: `waitset` is live by the caller; `cycle` is a local.
    let status = unsafe {
        hypha_cycle_create(
            waitset,
            name.as_ptr(),
            name.as_bytes().len(),
            period_nanos,
            Some(count_step),
            ptr::null_mut(),
            Some(count_release),
            &raw mut cycle,
        )
    };
    (status, cycle)
}

/// A source over `subscriber` (null: an unwired source).
///
/// # Safety
/// `waitset` must be live, `subscriber` null or live.
unsafe fn create_source(
    waitset: *mut HyphaWaitSet,
    subscriber: *mut HyphaSubscriber,
) -> (HyphaStatus, *mut HyphaInputSource) {
    let mut source = ptr::dangling_mut::<HyphaInputSource>();
    // SAFETY: as the caller guarantees; `source` is a local.
    let status = unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) };
    (status, source)
}

fn latest() -> HyphaCycleBindingOptions {
    HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 0,
        optional: 1,
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

/// Publish on `publisher` until the cycle stepped `at_least` times, within a bound.
///
/// # Safety
/// `publisher` must be live.
unsafe fn steps_while_publishing(publisher: *mut HyphaPublisher, at_least: u64) {
    let started = Instant::now();
    while STEPS.load(Ordering::SeqCst) < at_least {
        assert!(started.elapsed() < Duration::from_secs(10), "the cycle made no step");
        // SAFETY: live by the caller.
        unsafe { send_payload(publisher, &[1]) };
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The calls that only the thread of a run may make, from another thread during it.
struct Refused {
    waitset: *mut HyphaWaitSet,
    node: *mut HyphaNode,
    pending: *mut HyphaCycle,
    running: *mut HyphaCycle,
    source: *mut HyphaInputSource,
    unwired: *mut HyphaInputSource,
    output: *mut HyphaPublisher,
    timer: HyphaAttachmentId,
    /// A cycle with neither rate nor trigger on an idle `WaitSet`: its commit is refused
    /// with a reason, which a refused create or commit then clears.
    unscheduled: *mut HyphaCycle,
}

impl Refused {
    /// Record a refusal reason on this thread.
    ///
    /// # Safety
    /// `unscheduled` must be live.
    unsafe fn record_a_reason(&self) {
        // SAFETY: live by the caller; its `WaitSet` does not run.
        assert_eq!(unsafe { hypha_cycle_commit(self.unscheduled) }, HyphaStatus::InvalidArgument);
        assert!(!last_error().is_empty());
    }

    /// Each call returns `InvalidState` and changes nothing.
    ///
    /// # Safety
    /// Every handle must be live, and the `WaitSet` must run on another thread.
    unsafe fn check(&self) {
        let invalid_state = HyphaStatus::InvalidState;

        // A second run fills no report.
        let mut report =
            HyphaWaitSetRunReport { reason: 7, status: 7, attachment: 7, slow_callbacks: 7 };
        // SAFETY: the `WaitSet` is live; `report` is a local.
        assert_eq!(unsafe { hypha_waitset_run(self.waitset, 0, &raw mut report) }, invalid_state);
        assert_eq!((report.reason, report.status, report.attachment), (7, 7, 7));

        // SAFETY: the `WaitSet` is live, the id is a live registration.
        assert_eq!(unsafe { hypha_waitset_detach(self.waitset, self.timer) }, invalid_state);
        let record = attach::timer(1_000_000, Some(no_tick), ptr::null_mut(), Some(count_release));
        let mut late = 9;
        // SAFETY: the `WaitSet` is live; `record` and `late` are locals.
        let attached =
            unsafe { hypha_waitset_attach(self.waitset, &raw const record, &raw mut late) };
        assert_eq!(attached, invalid_state);
        assert_eq!(late, 0, "a refused attach writes no id");

        // SAFETY: the cycle is live.
        unsafe { self.record_a_reason() };
        // SAFETY: the `WaitSet` is live.
        let (created, cycle) = unsafe { create_cycle(self.waitset, "late", 1_000_000) };
        assert_eq!(created, invalid_state);
        assert!(cycle.is_null(), "a refused create writes no handle");
        assert_eq!(last_error(), "", "a create clears the reason on entry");
        // SAFETY: as above.
        let (created, source) = unsafe { create_source(self.waitset, ptr::null_mut()) };
        assert_eq!(created, invalid_state);
        assert!(source.is_null(), "a refused create writes no handle");
        // SAFETY: the source is live and stays the caller's.
        assert_eq!(unsafe { hypha_input_source_destroy(self.unwired) }, invalid_state);

        // SAFETY: every handle is live by the caller.
        unsafe { self.check_configuration(self.pending) };
        // SAFETY: as above.
        unsafe { self.check_configuration(self.running) };
        // SAFETY: as above.
        unsafe { self.record_a_reason() };
        // SAFETY: as above.
        assert_eq!(unsafe { hypha_cycle_commit(self.pending) }, invalid_state);
        assert_eq!(last_error(), "", "a commit clears the reason on entry");
        // SAFETY: as above.
        assert_eq!(unsafe { hypha_cycle_destroy(self.pending) }, invalid_state);
        // SAFETY: as above.
        assert_eq!(unsafe { hypha_cycle_destroy(self.running) }, invalid_state);
    }

    /// # Safety
    /// As [`Refused::check`].
    unsafe fn check_configuration(&self, cycle: *mut HyphaCycle) {
        let invalid_state = HyphaStatus::InvalidState;
        let mut index = 9;
        let options = latest();
        // SAFETY: live handles and locals.
        let bound = unsafe {
            hypha_cycle_bind_input(cycle, self.unwired, &raw const options, &raw mut index)
        };
        assert_eq!((bound, index), (invalid_state, 0));
        index = 9;
        // SAFETY: as above; a refused bind leaves the publisher to the caller.
        let bound = unsafe { hypha_cycle_bind_output(cycle, self.output, 0, 1, &raw mut index) };
        assert_eq!((bound, index), (invalid_state, 0));
        // SAFETY: as above.
        assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, 0) }, invalid_state);
        // SAFETY: as above.
        assert_eq!(unsafe { hypha_cycle_set_host_clock(cycle, 0) }, invalid_state);
        // SAFETY: as above.
        assert_eq!(unsafe { hypha_cycle_set_class_based(cycle, 1) }, invalid_state);
        let driver = HyphaClockDriverOptions {
            start_nanos: 0,
            timeout_nanos: 1_000_000_000,
            real_time_factor: 0.0,
        };
        // SAFETY: as above.
        assert_eq!(
            unsafe { hypha_cycle_set_clock_driver(cycle, &raw const driver) },
            invalid_state
        );
        let participant = b"participant";
        // SAFETY: as above; the name comes with its length.
        let expected = unsafe {
            hypha_cycle_expect_participant(cycle, participant.as_ptr().cast(), participant.len())
        };
        assert_eq!(expected, invalid_state);
        index = 9;
        // SAFETY: as above.
        let bound = unsafe {
            hypha_cycle_bind_transforms(cycle, self.source, self.unwired, 1_000_000, &raw mut index)
        };
        assert_eq!((bound, index), (invalid_state, 0));
        let port = b"port";
        let request = HyphaInputSupervisionRequest {
            node: self.node,
            name: port.as_ptr().cast(),
            name_len: port.len(),
            unsupervised: 1,
            check_stagnation: 0,
            takes_topic: 0,
            ranges: ranges(),
        };
        let mut result = HyphaInputSupervisionResult::default();
        let mut detail_len = 9;
        // SAFETY: as above; no detail buffer, which an empty detail needs none of.
        let bound = unsafe {
            hypha_cycle_bind_input_supervision(
                cycle,
                0,
                &raw const request,
                &raw mut result,
                ptr::null_mut(),
                0,
                &raw mut detail_len,
            )
        };
        assert_eq!((bound, detail_len), (invalid_state, 0));
    }
}

#[test]
fn another_thread_reads_cycle_snapshots_and_is_refused_every_other_call_during_a_run() {
    let node = create_node(&unique("cross_refusals"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { create_waitset(node) };
    let topic = format!("/{}", unique("cross_refusals"));
    // SAFETY: as above.
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 8) };
    // SAFETY: as above; the source takes the subscriber.
    let (created, source) = unsafe { create_source(waitset, subscriber) };
    assert_eq!(created, HyphaStatus::Ok);
    // SAFETY: as above.
    let (created, unwired) = unsafe { create_source(waitset, ptr::null_mut()) };
    assert_eq!(created, HyphaStatus::Ok);
    // SAFETY: as above.
    let (output_publisher, output_subscriber) =
        unsafe { create_pubsub(node, &format!("/{}", unique("cross_output")), 8) };
    // SAFETY: the subscriber is live and unused.
    unsafe { hypha_subscriber_destroy(output_subscriber) };

    // SAFETY: `waitset` is live.
    let (created, running) = unsafe { create_cycle(waitset, "running", 1_000_000) };
    assert_eq!(created, HyphaStatus::Ok);
    let options = latest();
    let mut index = 9;
    // SAFETY: live handles and locals.
    let bound =
        unsafe { hypha_cycle_bind_input(running, source, &raw const options, &raw mut index) };
    assert_eq!((bound, index), (HyphaStatus::Ok, 0));
    // SAFETY: `running` is live.
    assert_eq!(unsafe { hypha_cycle_commit(running) }, HyphaStatus::Ok);
    // SAFETY: `waitset` is live.
    let (created, pending) = unsafe { create_cycle(waitset, "pending", 1_000_000) };
    assert_eq!(created, HyphaStatus::Ok);
    let record = attach::timer(1_000_000_000, Some(no_tick), ptr::null_mut(), None);
    let mut timer = 0;
    // SAFETY: `waitset` is live; `record` and `timer` are locals.
    let attached = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut timer) };
    assert_eq!(attached, HyphaStatus::Ok);
    let idle_node = create_node(&unique("cross_idle"));
    // SAFETY: `idle_node` is the live node just created.
    let idle = unsafe { create_waitset(idle_node) };
    // SAFETY: as above.
    let (created, unscheduled) = unsafe { create_cycle(idle, "unscheduled", 0) };
    assert_eq!(created, HyphaStatus::Ok);
    STEPS.store(0, Ordering::SeqCst);
    RELEASES.store(0, Ordering::SeqCst);

    let runner = run_on_another_thread(waitset);
    // SAFETY: `publisher` is live.
    unsafe { steps_while_publishing(publisher, 3) };

    // The snapshots are readable from this thread.
    let mut health = HyphaCycleHealth::default();
    // SAFETY: `running` is live; `health` is a local.
    assert_eq!(unsafe { hypha_cycle_health(running, &raw mut health) }, HyphaStatus::Ok);
    assert!(health.steps >= 1, "the health of a finished step is visible");
    let mut view = HyphaCycleBindingView::default();
    // SAFETY: as above.
    assert_eq!(unsafe { hypha_cycle_binding(running, 0, &raw mut view) }, HyphaStatus::Ok);
    // SAFETY: as above; the pending cycle has no binding.
    assert_eq!(
        unsafe { hypha_cycle_binding(pending, 0, &raw mut view) },
        HyphaStatus::InvalidArgument
    );

    let refused = Refused {
        waitset,
        node,
        pending,
        running,
        source,
        unwired,
        output: output_publisher,
        timer,
        unscheduled,
    };
    // SAFETY: every handle is live and the `WaitSet` runs on the other thread.
    unsafe { refused.check() };
    assert_eq!(RELEASES.load(Ordering::SeqCst), 0, "a refused call releases no context");
    // The handle of a source a cycle binds is not its last owner: it goes during the run,
    // and the cycle keeps the receiver and the history.
    // SAFETY: `source` is live and not used again.
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);

    // SAFETY: `waitset` is live.
    assert_eq!(unsafe { hypha_waitset_request_stop(waitset) }, HyphaStatus::Ok);
    let (status, report) = runner.join().expect("the run thread finished");
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED);

    // Nothing changed: the same calls succeed now, and each handle is still the caller's.
    // SAFETY: every handle is live and destroyed once.
    unsafe {
        assert_eq!(hypha_waitset_detach(waitset, timer), HyphaStatus::Ok);
        assert_eq!(hypha_cycle_destroy(pending), HyphaStatus::Ok);
        assert_eq!(hypha_cycle_destroy(running), HyphaStatus::Ok);
        assert_eq!(RELEASES.load(Ordering::SeqCst), 2, "each cycle released its context once");
        assert_eq!(hypha_input_source_destroy(unwired), HyphaStatus::Ok);
        hypha_publisher_destroy(output_publisher);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
        hypha_node_destroy(idle_node);
    }
}
