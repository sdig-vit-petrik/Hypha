//! The budget a callback is judged against: the node's `[log] slow_callback_ms`, else the
//! period of a timer, else 10 ms for a subscription (#128). The count in the run report is
//! the observable; the lines are tested where they are built (`hypha_core`).
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;
use crate::common::{attach, c_text, create_node_with_file, unique};

use std::os::raw::c_void;
use std::path::Path;
use std::ptr;
use std::time::Duration;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

const MILLISECOND: u64 = 1_000_000;
const MD5: &str = "0123456789abcdef0123456789abcdef";

/// A node whose parameter file holds `parameters`; the file is read at creation.
fn node_with(label: &str, parameters: &str) -> *mut HyphaNode {
    let path = std::env::temp_dir().join(unique("slow_callback.toml"));
    std::fs::write(&path, parameters).expect("write parameter fixture");
    let node = create_node_with_file(&unique(label), Some(Path::new(&path)));
    std::fs::remove_file(&path).expect("remove parameter fixture");
    node
}

fn waitset_of(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    waitset
}

/// Sleeps as many milliseconds as its context holds.
extern "C" fn sleep_context_ms(_missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    std::thread::sleep(Duration::from_millis(context.addr() as u64));
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// A timer of `period_ms` whose callback takes `takes_ms`, run for `run_ms`; the number of
/// slow callbacks the run reports.
fn slow_timer_callbacks(parameters: &str, period_ms: u64, takes_ms: usize, run_ms: u64) -> u64 {
    let node = node_with("timer", parameters);
    let waitset = waitset_of(node);
    let record = attach::timer(
        period_ms.saturating_mul(MILLISECOND),
        Some(sleep_context_ms),
        ptr::without_provenance_mut(takes_ms),
        None,
    );
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    let slow = run(waitset, run_ms);
    unsafe { hypha_node_destroy(node) };
    slow
}

fn run(waitset: *mut HyphaWaitSet, run_ms: u64) -> u64 {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    let status =
        unsafe { hypha_waitset_run(waitset, run_ms.saturating_mul(MILLISECOND), &raw mut report) };
    assert_eq!(status, HyphaStatus::Ok);
    report.slow_callbacks
}

#[test]
fn a_10_hz_callback_that_takes_45_ms_of_its_100_ms_is_not_slow() {
    assert_eq!(slow_timer_callbacks("", 100, 45, 350), 0);
}

#[test]
fn a_timer_callback_that_outlasts_its_period_is_slow() {
    assert!(slow_timer_callbacks("", 20, 45, 200) > 0);
}

#[test]
fn a_node_that_sets_50_ms_does_not_warn_for_a_20_ms_callback_and_does_for_120_ms() {
    let budget = "[log]\nslow_callback_ms = 50\n";
    assert_eq!(slow_timer_callbacks(budget, 5, 20, 150), 0, "20 ms is inside 50 ms");
    assert!(slow_timer_callbacks(budget, 200, 120, 350) > 0, "120 ms is outside 50 ms");
}

#[test]
fn zero_writes_no_line_and_the_counter_still_judges_against_the_default() {
    assert!(slow_timer_callbacks("[log]\nslow_callback_ms = 0\n", 5, 20, 150) > 0);
}

#[test]
fn nodes_of_one_process_may_set_different_budgets() {
    let first = node_with("first", "[log]\nslow_callback_ms = 50\n");
    let second = node_with("second", "[log]\nslow_callback_ms = 5\n");
    unsafe { hypha_node_destroy(first) };
    unsafe { hypha_node_destroy(second) };
}

extern "C" fn sleep_on_data(
    _delivery: *const HyphaDelivery,
    context: *mut c_void,
) -> HyphaCallbackResult {
    std::thread::sleep(Duration::from_millis(context.addr() as u64));
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
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

/// A subscription whose callback takes `takes_ms` for the one sample published; the number
/// of slow callbacks the run reports.
fn slow_subscription_callbacks(parameters: &str, takes_ms: usize) -> u64 {
    let node = node_with("subscription", parameters);
    let waitset = waitset_of(node);
    let topic = c_text(&format!("/{}", unique("slow_callback")));
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
        Some(sleep_on_data),
        ptr::without_provenance_mut(takes_ms),
        None,
    );
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    unsafe { hypha_subscriber_destroy(subscriber) };
    unsafe { publish(publisher, &7_u32.to_le_bytes()) };
    let slow = run(waitset, 400);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    slow
}

#[test]
fn a_subscription_callback_is_slow_over_10_ms_unless_the_node_sets_a_budget() {
    assert_eq!(slow_subscription_callbacks("", 30), 1);
    assert_eq!(slow_subscription_callbacks("[log]\nslow_callback_ms = 50\n", 30), 0);
    assert_eq!(slow_subscription_callbacks("[log]\nslow_callback_ms = 50\n", 120), 1);
}
