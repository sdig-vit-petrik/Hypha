//! An event-driven subscription wakes its `WaitSet` for its data, not on a clock.
//! The 50 ms audit also reads the queue, so a publisher that does not notify is still served.
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

/// Nanoseconds from [`epoch`] to the last delivery.
static DELIVERED_AT_NANOS: AtomicU64 = AtomicU64::new(0);

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
    let since_start = u64::try_from(epoch().elapsed().as_nanos()).unwrap_or(u64::MAX);
    DELIVERED_AT_NANOS.store(since_start, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
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
fn an_idle_event_driven_subscription_wakes_only_for_the_audit() {
    let deliveries = AtomicU64::new(0);
    let fixture = fixture("idle_event", &deliveries);
    let before = sweeps(fixture.node);
    run_for(fixture.waitset, 1_000_000_000);
    let turns = sweeps(fixture.node) - before;
    // The 50 ms audit alone: about 20 turns. A 1 ms poll would take about 1000.
    assert!((10..=60).contains(&turns), "an idle notified subscription took {turns} turns in 1 s");
    assert_eq!(deliveries.load(Ordering::SeqCst), 0);
}

#[test]
fn a_sample_wakes_its_subscription_well_before_the_audit() {
    let deliveries = AtomicU64::new(0);
    let fixture = fixture("prompt_event", &deliveries);
    let publisher = fixture.publisher as usize;
    let start = epoch();
    let sent = std::thread::scope(|scope| {
        let sender = scope.spawn(|| {
            // Between two 50 ms audits, so only the notification can deliver it at once.
            std::thread::sleep(std::time::Duration::from_millis(120));
            let sent = start.elapsed();
            unsafe { publish(publisher as *mut HyphaPublisher, &[7]) };
            sent
        });
        run_for(fixture.waitset, 300_000_000);
        sender.join().expect("the sender thread")
    });
    assert_eq!(deliveries.load(Ordering::SeqCst), 1);
    // The callback stamps the delivery, so a descheduled sender cannot inflate it.
    let delivered = DELIVERED_AT_NANOS.load(Ordering::SeqCst);
    let latency = std::time::Duration::from_nanos(delivered).saturating_sub(sent);
    assert!(
        latency < std::time::Duration::from_millis(15),
        "the sample waited {latency:?}, as long as a poll or an audit"
    );
}

/// Nanoseconds from [`epoch`] to the first call of the late-start timer.
static TIMER_FIRST_CALL_NANOS: AtomicU64 = AtomicU64::new(0);
/// How many times the late-start timer ran.
static TIMER_CALLS: AtomicU64 = AtomicU64::new(0);

extern "C" fn record_timer_call(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    let calls = TIMER_CALLS.fetch_add(1, Ordering::SeqCst);
    if calls == 0 {
        let since_start = u64::try_from(epoch().elapsed().as_nanos()).unwrap_or(u64::MAX);
        TIMER_FIRST_CALL_NANOS.store(since_start, Ordering::SeqCst);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_host_timer_fires_within_five_milliseconds_of_its_deadline_when_the_run_starts_late() {
    // A host timer starts its periods at the first run turn, so the time before the run is
    // not missed: its deadline is R + 100 ms.
    let node = common::create_unique_node("late_timer");
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let record = attach::timer(100_000_000, Some(record_timer_call), ptr::null_mut(), None);
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    std::thread::sleep(std::time::Duration::from_millis(37));
    let run_started_nanos = u64::try_from(epoch().elapsed().as_nanos()).unwrap_or(u64::MAX);
    run_for(waitset, 300_000_000);

    assert!(TIMER_CALLS.load(Ordering::SeqCst) >= 1, "the timer never ran");
    let first_call = TIMER_FIRST_CALL_NANOS.load(Ordering::SeqCst);
    let lateness_nanos = first_call.saturating_sub(run_started_nanos);
    assert!(
        (100_000_000..105_000_000).contains(&lateness_nanos),
        "the first call came {} ms after the run started; its deadline is 100 ms",
        lateness_nanos / 1_000_000
    );
    unsafe { hypha_node_destroy(node) };
}
