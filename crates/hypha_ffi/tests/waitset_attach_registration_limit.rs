//! A `WaitSet` accepts 128 registrations; an attach past that is refused with `ResourceLimit`.
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

const MAX_REGISTRATIONS: usize = 128;

extern "C" fn ignore_interval(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

unsafe fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    waitset
}

unsafe fn attach_timer(waitset: *mut HyphaWaitSet) -> (HyphaStatus, HyphaAttachmentId) {
    let mut attachment: HyphaAttachmentId = 0;
    let status = unsafe {
        hypha_waitset_attach(
            waitset,
            &attach::timer(1_000_000, Some(ignore_interval), ptr::null_mut(), None),
            &raw mut attachment,
        )
    };
    (status, attachment)
}

#[test]
fn a_timer_is_refused_once_the_table_is_full() {
    let node = create_node(&unique("interval"));
    let waitset = unsafe { create_waitset(node) };

    // The node's own housekeeping already holds a few slots, so the table fills
    // before the caller has attached the whole limit.
    let mut accepted = 0;
    let refused = loop {
        let (status, attachment) = unsafe { attach_timer(waitset) };
        if status != HyphaStatus::Ok {
            break (status, attachment);
        }
        accepted += 1;
        assert!(accepted < MAX_REGISTRATIONS, "the table must fill at the documented limit");
    };

    assert_eq!(refused.0, HyphaStatus::ResourceLimit, "a full table must refuse another timer");
    assert_eq!(refused.1, 0, "a refused attachment reports no identifier");
    assert!(accepted > MAX_REGISTRATIONS - 8, "only the housekeeping is ahead of the caller");
    unsafe { hypha_node_destroy(node) };
}

extern "C" fn count_release(context: *mut c_void) {
    // SAFETY: each context of this release is an `AtomicU64` the test keeps alive.
    let counter = unsafe { &*context.cast::<AtomicU64>() };
    counter.fetch_add(1, Ordering::SeqCst);
}

fn counter_context(counter: &AtomicU64) -> *mut c_void {
    ptr::from_ref(counter).cast_mut().cast()
}

extern "C" fn ignore_delivery(
    _delivery: *const HyphaDelivery,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
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

#[test]
fn an_attach_refused_at_the_registration_limit_leaves_the_context_to_the_caller() {
    let node = create_node(&unique("limit_context"));
    let waitset = unsafe { create_waitset(node) };
    // SAFETY: `node` is the live node just created.
    let (publisher, subscriber) =
        unsafe { common::replay::create_pubsub(node, &format!("/{}", unique("limit_context")), 4) };
    let mut fillers = Vec::new();
    loop {
        let (status, id) = unsafe { attach_timer(waitset) };
        if status == HyphaStatus::ResourceLimit {
            break;
        }
        assert_eq!(status, HyphaStatus::Ok, "an attach below the limit is accepted");
        fillers.push(id);
    }

    let timer_releases = AtomicU64::new(0);
    let subscription_releases = AtomicU64::new(0);
    let timer = attach::timer(
        1_000_000,
        Some(ignore_interval),
        counter_context(&timer_releases),
        Some(count_release),
    );
    let subscription = attach::subscription(
        subscriber,
        Some(ignore_delivery),
        counter_context(&subscription_releases),
        Some(count_release),
    );
    // SAFETY: the handles are live; the counters outlive the node.
    let (timer_status, timer_id) = unsafe { attach_record(waitset, &timer) };
    // SAFETY: as above.
    let (subscription_status, subscription_id) = unsafe { attach_record(waitset, &subscription) };
    assert_eq!(timer_status, HyphaStatus::ResourceLimit);
    assert_eq!(subscription_status, HyphaStatus::ResourceLimit);
    assert_eq!((timer_id, subscription_id), (0, 0), "a refused attach fills no id");
    let releases =
        (timer_releases.load(Ordering::SeqCst), subscription_releases.load(Ordering::SeqCst));
    assert_eq!(releases, (0, 0), "a refused timer and subscription are not released");

    // The caller still owns both contexts: with room again, the same records are accepted,
    // and each context is released once, at destroy.
    for id in fillers.drain(..2) {
        assert_eq!(unsafe { hypha_waitset_detach(waitset, id) }, HyphaStatus::Ok);
    }
    // SAFETY: as above.
    assert_eq!(unsafe { attach_record(waitset, &timer) }.0, HyphaStatus::Ok);
    // SAFETY: as above.
    assert_eq!(unsafe { attach_record(waitset, &subscription) }.0, HyphaStatus::Ok);
    // SAFETY: every handle is live and destroyed once, the node last.
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    assert_eq!(timer_releases.load(Ordering::SeqCst), 1);
    assert_eq!(subscription_releases.load(Ordering::SeqCst), 1);
}
