//! C ABI tests for `hypha_waitset_attach`: refusals by kind and ownership of `context`.
//! A field a refusal must not read holds a dangling pointer.
#![allow(clippy::expect_used)]

mod common;
use crate::common::attach;
use crate::common::create_node;
use crate::common::unique;

use std::os::raw::c_void;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

/// Counts the releases of the context that points to it.
extern "C" fn count_release(context: *mut c_void) {
    let counter = unsafe { &*context.cast::<AtomicU64>() };
    counter.fetch_add(1, Ordering::SeqCst);
}

fn counter_context(counter: &AtomicU64) -> *mut c_void {
    ptr::from_ref(counter).cast_mut().cast()
}

extern "C" fn no_op_interval(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn no_op_delivery(
    _delivery: *const HyphaDelivery,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

unsafe fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    let status = unsafe { hypha_node_default_waitset(node, &raw mut waitset) };
    assert_eq!(status, HyphaStatus::Ok);
    waitset
}

/// A pointer that a refused record must never read.
fn unread_pointer<T>() -> *mut T {
    NonNull::<T>::dangling().as_ptr()
}

#[test]
fn an_unknown_kind_is_refused_and_writes_no_identifier() {
    let node = create_node(&unique("entry_kind"));
    let waitset = unsafe { create_waitset(node) };
    let record = attach::record(99);
    let mut out: HyphaAttachmentId = 77;

    let status = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut out) };

    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert_eq!(out, 0);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn an_unknown_attachment_kind_is_refused() {
    // A record that is a valid timer in every field but its kind is refused, and the caller
    // keeps its context.
    let node = create_node(&unique("entry_kind_one"));
    let waitset = unsafe { create_waitset(node) };
    let releases = AtomicU64::new(0);
    let mut record = attach::timer(
        1_000_000,
        Some(no_op_interval),
        counter_context(&releases),
        Some(count_release),
    );
    record.kind = 1;
    let mut out: HyphaAttachmentId = 77;

    let status = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut out) };
    unsafe { hypha_node_destroy(node) };

    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert_eq!(out, 0, "a refused attach fills no id");
    assert_eq!(releases.load(Ordering::SeqCst), 0, "a refused attach must not release");
}

#[test]
fn a_zero_timer_period_is_refused() {
    let node = create_node(&unique("entry_period"));
    let waitset = unsafe { create_waitset(node) };
    let timer = attach::timer(0, Some(no_op_interval), ptr::null_mut(), None);
    let mut out: HyphaAttachmentId = 0;

    let timer_status = unsafe { hypha_waitset_attach(waitset, &raw const timer, &raw mut out) };

    assert_eq!(timer_status, HyphaStatus::InvalidArgument);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_timer_period_framework_time_cannot_hold_is_refused() {
    let node = create_node(&unique("entry_timer_max"));
    let waitset = unsafe { create_waitset(node) };
    let timer = attach::timer(u64::MAX, Some(no_op_interval), ptr::null_mut(), None);
    let mut out: HyphaAttachmentId = 0;

    let status = unsafe { hypha_waitset_attach(waitset, &raw const timer, &raw mut out) };

    assert_eq!(status, HyphaStatus::InvalidArgument);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_refused_attach_keeps_the_context_with_the_caller() {
    let node = create_node(&unique("entry_refused_context"));
    let waitset = unsafe { create_waitset(node) };
    let releases = AtomicU64::new(0);
    let record =
        attach::timer(0, Some(no_op_interval), counter_context(&releases), Some(count_release));
    let mut out: HyphaAttachmentId = 7;

    let status = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut out) };
    unsafe { hypha_node_destroy(node) };

    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert_eq!(out, 0, "a refused attach fills no id");
    assert_eq!(releases.load(Ordering::SeqCst), 0, "a refused attach must not release");
}

#[test]
fn an_accepted_attach_releases_its_context_once_at_destroy() {
    let node = create_node(&unique("entry_owned_context"));
    let waitset = unsafe { create_waitset(node) };
    let releases = AtomicU64::new(0);
    let record = attach::timer(
        1_000_000,
        Some(no_op_interval),
        counter_context(&releases),
        Some(count_release),
    );
    let mut out: HyphaAttachmentId = 0;

    let status = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut out) };
    assert_eq!(status, HyphaStatus::Ok);
    assert_ne!(out, 0);
    assert_eq!(releases.load(Ordering::SeqCst), 0, "no release while attached");

    unsafe { hypha_node_destroy(node) };
    assert_eq!(releases.load(Ordering::SeqCst), 1, "destroy releases exactly once");
}

#[test]
fn a_subscription_with_a_periodic_field_is_refused() {
    let node = create_node(&unique("stray_subscription"));
    let waitset = unsafe { create_waitset(node) };
    let mut record =
        attach::subscription(unread_pointer(), Some(no_op_delivery), ptr::null_mut(), None);
    record.period_nanos = 1_000_000;
    let mut out: HyphaAttachmentId = 0;

    let status = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut out) };

    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert_eq!(out, 0);
    unsafe { hypha_node_destroy(node) };
}

// A callback that is required for a kind must be present.

#[test]
fn a_kind_without_its_callback_is_refused() {
    let node = create_node(&unique("nocb"));
    let waitset = unsafe { create_waitset(node) };
    let records = [
        ("subscription", attach::subscription(unread_pointer(), None, ptr::null_mut(), None)),
        ("timer", attach::timer(1_000_000, None, ptr::null_mut(), None)),
    ];
    for (kind, record) in records {
        let mut out: HyphaAttachmentId = 0;
        let status = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut out) };
        assert_eq!(status, HyphaStatus::InvalidArgument, "{kind} without a callback");
        assert_eq!(out, 0, "{kind}: a refused attach fills no id");
    }
    unsafe { hypha_node_destroy(node) };
}

/// A publisher and a subscriber of the live `node` on a fresh topic named after `label`.
fn pubsub(node: *mut HyphaNode, label: &str) -> (*mut HyphaPublisher, *mut HyphaSubscriber) {
    // SAFETY: every caller passes the live node it just created.
    unsafe { common::replay::create_pubsub(node, &format!("/{}", unique(label)), 4) }
}

/// # Safety
/// `waitset` must be live, and the record's handles and context as the attach requires.
unsafe fn attach_record(
    waitset: *mut HyphaWaitSet,
    record: &HyphaAttachment,
) -> (HyphaStatus, HyphaAttachmentId) {
    let mut out: HyphaAttachmentId = 77;
    // SAFETY: as the caller guarantees; `record` and `out` are live.
    let status = unsafe { hypha_waitset_attach(waitset, record, &raw mut out) };
    (status, out)
}

/// What a timer callback tries inside the run, and what it got.
struct AttachInsideRun {
    waitset: *mut HyphaWaitSet,
    subscriber: *mut HyphaSubscriber,
    timer_releases: AtomicU64,
    subscription_releases: AtomicU64,
    timer_status: AtomicI32,
    subscription_status: AtomicI32,
    written_ids: AtomicU64,
}

/// Inside the run: attach a counted timer and a counted subscription, record their
/// statuses, and stop the run.
extern "C" fn attach_inside_the_run(_missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    // SAFETY: the context is the test's `AttachInsideRun`, alive until the node goes.
    let state = unsafe { &*context.cast::<AttachInsideRun>() };
    let timer = attach::timer(
        1_000_000,
        Some(no_op_interval),
        counter_context(&state.timer_releases),
        Some(count_release),
    );
    let subscription = attach::subscription(
        state.subscriber,
        Some(no_op_delivery),
        counter_context(&state.subscription_releases),
        Some(count_release),
    );
    // SAFETY: the `WaitSet` and the subscriber are live; the counters outlive the node.
    let (timer_status, timer_id) = unsafe { attach_record(state.waitset, &timer) };
    // SAFETY: as above.
    let (subscription_status, subscription_id) =
        unsafe { attach_record(state.waitset, &subscription) };
    state.timer_status.store(timer_status as i32, Ordering::SeqCst);
    state.subscription_status.store(subscription_status as i32, Ordering::SeqCst);
    state.written_ids.store(timer_id | subscription_id, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_STOP, status: 0 }
}

#[test]
fn an_attach_refused_inside_a_run_leaves_the_context_to_the_caller() {
    let node = create_node(&unique("entry_refused_running"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) = pubsub(node, "entry_refused_running");
    let state = AttachInsideRun {
        waitset,
        subscriber,
        timer_releases: AtomicU64::new(0),
        subscription_releases: AtomicU64::new(0),
        timer_status: AtomicI32::new(i32::MIN),
        subscription_status: AtomicI32::new(i32::MIN),
        written_ids: AtomicU64::new(u64::MAX),
    };
    let context = ptr::from_ref(&state).cast_mut().cast::<c_void>();
    let driver = attach::timer(1_000_000, Some(attach_inside_the_run), context, None);
    // SAFETY: the `WaitSet` is live; `state` outlives the node.
    let (status, _driver) = unsafe { attach_record(waitset, &driver) };
    assert_eq!(status, HyphaStatus::Ok);

    let mut report =
        HyphaWaitSetRunReport { reason: 99, status: 0, attachment: 0, slow_callbacks: 0 };
    // SAFETY: the `WaitSet` is live; `report` is a local.
    let run = unsafe { hypha_waitset_run(waitset, 2_000_000_000, &raw mut report) };
    assert_eq!(run, HyphaStatus::Ok);
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED, "the callback ran and stopped the run");
    let invalid_state = HyphaStatus::InvalidState as i32;
    assert_eq!(state.timer_status.load(Ordering::SeqCst), invalid_state, "timer inside a run");
    assert_eq!(
        state.subscription_status.load(Ordering::SeqCst),
        invalid_state,
        "subscription inside a run"
    );
    assert_eq!(state.written_ids.load(Ordering::SeqCst), 0, "a refused attach fills no id");
    let releases = (
        state.timer_releases.load(Ordering::SeqCst),
        state.subscription_releases.load(Ordering::SeqCst),
    );
    assert_eq!(releases, (0, 0), "a refused timer and subscription are not released");

    // The caller still owns both contexts, and the subscriber is not claimed: the same
    // records are accepted after the run, and each context is released once, at destroy.
    let timer = attach::timer(
        1_000_000,
        Some(no_op_interval),
        counter_context(&state.timer_releases),
        Some(count_release),
    );
    let subscription = attach::subscription(
        subscriber,
        Some(no_op_delivery),
        counter_context(&state.subscription_releases),
        Some(count_release),
    );
    // SAFETY: the handles are live; the counters outlive the node.
    assert_eq!(unsafe { attach_record(waitset, &timer) }.0, HyphaStatus::Ok);
    // SAFETY: as above.
    assert_eq!(unsafe { attach_record(waitset, &subscription) }.0, HyphaStatus::Ok);
    // SAFETY: every handle is live and destroyed once, the node last.
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    assert_eq!(state.timer_releases.load(Ordering::SeqCst), 1);
    assert_eq!(state.subscription_releases.load(Ordering::SeqCst), 1);
}

#[test]
fn an_attach_of_a_subscriber_another_registration_dispatches_leaves_the_context_to_the_caller() {
    let node = create_node(&unique("entry_refused_dispatched"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) = pubsub(node, "entry_refused_dispatched");
    let first = AtomicU64::new(0);
    let second = AtomicU64::new(0);
    let dispatching = attach::subscription(
        subscriber,
        Some(no_op_delivery),
        counter_context(&first),
        Some(count_release),
    );
    // SAFETY: the handles are live; the counters outlive the node.
    assert_eq!(unsafe { attach_record(waitset, &dispatching) }.0, HyphaStatus::Ok);

    let again = attach::subscription(
        subscriber,
        Some(no_op_delivery),
        counter_context(&second),
        Some(count_release),
    );
    // SAFETY: as above.
    let (status, id) = unsafe { attach_record(waitset, &again) };
    assert_eq!(status, HyphaStatus::InvalidState, "one registration dispatches a subscriber");
    assert_eq!(id, 0, "a refused attach fills no id");
    assert_eq!(second.load(Ordering::SeqCst), 0, "a refused attach must not release");

    // SAFETY: every handle is live and destroyed once, the node last.
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    assert_eq!(first.load(Ordering::SeqCst), 1, "the dispatching registration is released once");
    assert_eq!(second.load(Ordering::SeqCst), 0, "nothing ever releases a refused context");
}

#[test]
fn an_attach_of_a_subscriber_an_input_source_holds_leaves_the_context_to_the_caller() {
    let node = create_node(&unique("entry_refused_sourced"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) = pubsub(node, "entry_refused_sourced");
    let mut source = ptr::null_mut();
    // SAFETY: the handles are live; the source takes over the caller's subscriber handle.
    let created = unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) };
    assert_eq!(created, HyphaStatus::Ok);
    let releases = AtomicU64::new(0);
    let record = attach::subscription(
        subscriber,
        Some(no_op_delivery),
        counter_context(&releases),
        Some(count_release),
    );
    // SAFETY: the source holds the subscriber alive; the counter outlives the node.
    let (status, id) = unsafe { attach_record(waitset, &record) };
    assert_eq!(status, HyphaStatus::InvalidOperation, "a source's subscriber is not attached");
    assert_eq!(id, 0, "a refused attach fills no id");
    assert_eq!(releases.load(Ordering::SeqCst), 0, "a refused attach must not release");

    // SAFETY: every handle is live and destroyed once, the node last.
    unsafe {
        assert_eq!(hypha_input_source_destroy(source), HyphaStatus::Ok);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    assert_eq!(releases.load(Ordering::SeqCst), 0, "nothing ever releases a refused context");
}
