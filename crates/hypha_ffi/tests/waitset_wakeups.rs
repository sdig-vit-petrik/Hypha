//! An event-driven subscription wakes its `WaitSet` for its data, not on a clock: nothing
//! polls its queue.
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;
use crate::common::{attach, c_text, unique};

use std::os::raw::c_void;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

const MD5: &str = "0123456789abcdef0123456789abcdef";

/// One process-wide origin, shared by the test and the callback.
fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

extern "C" fn count_data(
    _delivery: *const HyphaDelivery,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let deliveries = unsafe { &*context.cast::<AtomicU64>() };
    deliveries.fetch_add(1, Ordering::SeqCst);
    // A delivery ends the run, so its stop reason shows what woke it.
    HyphaCallbackResult { action: HYPHA_ACTION_STOP, status: 0 }
}

struct Fixture {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    publisher: *mut HyphaPublisher,
}

fn fixture(label: &str, deliveries: &AtomicU64) -> Fixture {
    let node = common::create_unique_node(label);
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let topic = c_text(&format!("/{}", unique(label)));
    let data_type = c_text("std_msgs/Counter");
    let md5 = c_text(MD5);
    let pub_qos =
        HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
    let mut publisher = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publish_with_qos(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                data_type.as_ptr(),
                data_type.as_bytes().len(),
                md5.as_ptr(),
                md5.as_bytes().len(),
                c"".as_ptr(),
                0,
                &raw const pub_qos,
                &raw mut publisher,
            )
        },
        HyphaStatus::Ok
    );
    let sub_qos = HyphaSubscriberOptions {
        depth: 8,
        no_publisher_warning_after_nanos: 0,
        qos: hypha_ffi::HyphaQos::default(),
    };
    let mut subscriber = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_subscribe_typed(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                ptr::null(),
                0,
                md5.as_ptr(),
                md5.as_bytes().len(),
                ptr::null(),
                0,
                &raw const sub_qos,
                &raw mut subscriber,
            )
        },
        HyphaStatus::Ok
    );
    let record = attach::subscription(
        subscriber,
        Some(count_data),
        (&raw const *deliveries).cast_mut().cast(),
        None,
    );
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    unsafe { hypha_subscriber_destroy(subscriber) };
    Fixture { node, waitset, publisher }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            hypha_publisher_destroy(self.publisher);
            hypha_node_destroy(self.node);
        }
    }
}

fn run_for(waitset: *mut HyphaWaitSet, nanos: u64) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, nanos, &raw mut report) }, HyphaStatus::Ok);
    assert_eq!(report.reason, HYPHA_RUN_REASON_DEADLINE);
}

/// Run with a limit that only guards against a hang; a callback must end the run.
fn run_until_stopped(waitset: *mut HyphaWaitSet) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(
        unsafe { hypha_waitset_run(waitset, 60_000_000_000, &raw mut report) },
        HyphaStatus::Ok
    );
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED, "no callback ended the run");
}

fn sweeps(node: *const HyphaNode) -> u64 {
    common::node_metric(node, "node.sweeps").expect("the node emits node.sweeps")
}

unsafe fn publish(publisher: *mut HyphaPublisher, payload: &[u8]) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publisher_loan_frame(publisher, payload.len(), &raw mut loan, &raw mut bytes)
        },
        HyphaStatus::Ok
    );
    unsafe { ptr::copy_nonoverlapping(payload.as_ptr(), bytes, payload.len()) };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
}

#[test]
fn an_idle_event_driven_subscription_does_not_wake_the_run() {
    let deliveries = AtomicU64::new(0);
    let fixture = fixture("idle_event", &deliveries);
    let before = sweeps(fixture.node);
    run_for(fixture.waitset, 1_000_000_000);
    let turns = sweeps(fixture.node) - before;
    // The first turn and at most one 1 s backstop; a 50 ms poll would take about 20.
    assert!(turns <= 3, "an idle notified subscription took {turns} turns in 1 s");
    assert_eq!(deliveries.load(Ordering::SeqCst), 0);
}

#[test]
fn a_sample_wakes_its_subscription_at_once() {
    let deliveries = AtomicU64::new(0);
    let fixture = fixture("prompt_event", &deliveries);
    let publisher = fixture.publisher as usize;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            // After the run has started and found nothing: only the notification can wake it.
            std::thread::sleep(std::time::Duration::from_millis(120));
            unsafe { publish(publisher as *mut HyphaPublisher, &[7]) };
        });
        run_until_stopped(fixture.waitset);
    });
    assert_eq!(deliveries.load(Ordering::SeqCst), 1);
}

/// Nanoseconds from [`epoch`] to the first call of the late-start timer.
static TIMER_FIRST_CALL_NANOS: AtomicU64 = AtomicU64::new(0);
/// How many times the late-start timer ran.
static TIMER_CALLS: AtomicU64 = AtomicU64::new(0);

/// The periods the late-start timer reported missed at its first call.
static TIMER_FIRST_MISSED: AtomicU64 = AtomicU64::new(u64::MAX);

extern "C" fn record_timer_call(missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    let calls = TIMER_CALLS.fetch_add(1, Ordering::SeqCst);
    if calls == 0 {
        let since_start = u64::try_from(epoch().elapsed().as_nanos()).unwrap_or(u64::MAX);
        TIMER_FIRST_CALL_NANOS.store(since_start, Ordering::SeqCst);
        TIMER_FIRST_MISSED.store(missed, Ordering::SeqCst);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_STOP, status: 0 }
}

#[test]
fn a_host_timer_starts_its_periods_at_the_run_not_at_the_attach() {
    // A host timer starts its periods at the first run turn: the 150 ms before the run,
    // longer than its period, are not missed, and its first deadline is R + 100 ms.
    let node = common::create_unique_node("late_timer");
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let record = attach::timer(100_000_000, Some(record_timer_call), ptr::null_mut(), None);
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    std::thread::sleep(std::time::Duration::from_millis(150));
    let run_started_nanos = u64::try_from(epoch().elapsed().as_nanos()).unwrap_or(u64::MAX);
    run_until_stopped(waitset);

    assert_eq!(TIMER_CALLS.load(Ordering::SeqCst), 1, "the first call ends the run");
    assert_eq!(TIMER_FIRST_MISSED.load(Ordering::SeqCst), 0, "time before the run was missed");
    // Never early: a timer anchored at the attach would be due at once. Late is the host.
    let since_run = TIMER_FIRST_CALL_NANOS.load(Ordering::SeqCst).saturating_sub(run_started_nanos);
    assert!(
        since_run >= 100_000_000,
        "the first call came {} ms after the run started, before its 100 ms deadline",
        since_run / 1_000_000
    );
    unsafe { hypha_node_destroy(node) };
}
