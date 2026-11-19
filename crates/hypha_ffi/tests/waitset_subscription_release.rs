//! A subscription's context is released once, after its subscriber stopped dispatching.
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;
use common::attach;
use common::replay::{create_pubsub, create_waitset};
use common::{create_node, unique};

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

extern "C" fn ignore_delivery(
    _delivery: *const HyphaDelivery,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn count_release(context: *mut c_void) {
    // SAFETY: each context of this release is an `AtomicU64` the test keeps alive.
    let counter = unsafe { &*context.cast::<AtomicU64>() };
    counter.fetch_add(1, Ordering::SeqCst);
}

/// What the re-attaching release needs, and what it found.
struct Reattach {
    waitset: *mut HyphaWaitSet,
    subscriber: *mut HyphaSubscriber,
    releases: AtomicU64,
    status: AtomicI32,
}

/// Attaches the released registration's subscriber again, and records the status.
extern "C" fn reattach_on_release(context: *mut c_void) {
    // SAFETY: the context is the test's `Reattach`, alive until the node is destroyed.
    let reattach = unsafe { &*context.cast::<Reattach>() };
    reattach.releases.fetch_add(1, Ordering::SeqCst);
    let record =
        attach::subscription(reattach.subscriber, Some(ignore_delivery), ptr::null_mut(), None);
    let mut id = 0;
    // SAFETY: the `WaitSet` and the subscriber are live; `record` and `id` are locals.
    let status = unsafe { hypha_waitset_attach(reattach.waitset, &raw const record, &raw mut id) };
    reattach.status.store(status as i32, Ordering::SeqCst);
}

/// # Safety
/// `waitset` and `subscriber` must be live, and `context` must outlive the registration.
unsafe fn attach_with(
    waitset: *mut HyphaWaitSet,
    subscriber: *mut HyphaSubscriber,
    context: *mut c_void,
    release: HyphaContextRelease,
) -> HyphaAttachmentId {
    let record = attach::subscription(subscriber, Some(ignore_delivery), context, release);
    let mut id = 0;
    // SAFETY: the handles are live by the caller; `record` and `id` are locals.
    let status = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut id) };
    assert_eq!(status, HyphaStatus::Ok);
    id
}

/// A publisher and a subscriber of the live `node` on a fresh topic named after `label`.
fn pubsub(node: *mut HyphaNode, label: &str) -> (*mut HyphaPublisher, *mut HyphaSubscriber) {
    // SAFETY: every caller passes the live node it just created.
    unsafe { create_pubsub(node, &format!("/{}", unique(label)), 4) }
}

#[test]
fn a_subscription_releases_its_context_once_at_detach_and_once_at_destroy() {
    let node = create_node(&unique("subscription_release"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { create_waitset(node) };
    let (first_publisher, first) = pubsub(node, "release_first");
    let (second_publisher, second) = pubsub(node, "release_second");
    let detached = AtomicU64::new(0);
    let destroyed = AtomicU64::new(0);
    let detached_context = std::ptr::from_ref(&detached).cast_mut().cast::<c_void>();
    let destroyed_context = std::ptr::from_ref(&destroyed).cast_mut().cast::<c_void>();
    // SAFETY: the handles are live; both counters outlive the node.
    let detached_id = unsafe { attach_with(waitset, first, detached_context, Some(count_release)) };
    // SAFETY: as above.
    let _kept = unsafe { attach_with(waitset, second, destroyed_context, Some(count_release)) };

    // SAFETY: `waitset` is live.
    assert_eq!(unsafe { hypha_waitset_detach(waitset, detached_id) }, HyphaStatus::Ok);
    assert_eq!(detached.load(Ordering::SeqCst), 1, "the detach releases its context once");
    assert_eq!(destroyed.load(Ordering::SeqCst), 0, "the other registration keeps its context");

    // SAFETY: every handle is live and destroyed once, the node last.
    unsafe {
        hypha_subscriber_destroy(first);
        hypha_subscriber_destroy(second);
        hypha_publisher_destroy(first_publisher);
        hypha_publisher_destroy(second_publisher);
        hypha_node_destroy(node);
    }
    assert_eq!(detached.load(Ordering::SeqCst), 1, "a detached context is not released again");
    assert_eq!(destroyed.load(Ordering::SeqCst), 1, "the destroy releases its context once");
}

#[test]
fn a_subscription_release_runs_after_its_registration_stopped_dispatching() {
    let node = create_node(&unique("release_after_dispatch"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { create_waitset(node) };
    let (publisher, subscriber) = pubsub(node, "release_after_dispatch");
    let reattach = Reattach {
        waitset,
        subscriber,
        releases: AtomicU64::new(0),
        status: AtomicI32::new(i32::MIN),
    };
    let context = std::ptr::from_ref(&reattach).cast_mut().cast::<c_void>();
    // SAFETY: the handles are live; `reattach` outlives the node.
    let id = unsafe { attach_with(waitset, subscriber, context, Some(reattach_on_release)) };

    // SAFETY: `waitset` is live.
    assert_eq!(unsafe { hypha_waitset_detach(waitset, id) }, HyphaStatus::Ok);
    assert_eq!(reattach.releases.load(Ordering::SeqCst), 1);
    assert_eq!(
        reattach.status.load(Ordering::SeqCst),
        HyphaStatus::Ok as i32,
        "inside the release no registration dispatches the subscriber any more"
    );

    // SAFETY: every handle is live and destroyed once, the node last.
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    assert_eq!(reattach.releases.load(Ordering::SeqCst), 1, "the re-attach has no release");
}

/// What the release that attaches elsewhere needs, and what it found.
struct AttachElsewhere {
    /// The `WaitSet` of a second node, not running.
    other: *mut HyphaWaitSet,
    subscriber: *mut HyphaSubscriber,
    releases: AtomicU64,
    status: AtomicI32,
}

/// Attaches the released registration's subscriber to the second node's `WaitSet`,
/// and detaches it there again.
extern "C" fn attach_elsewhere_on_release(context: *mut c_void) {
    // SAFETY: the context is the test's `AttachElsewhere`, alive until both nodes go.
    let state = unsafe { &*context.cast::<AttachElsewhere>() };
    state.releases.fetch_add(1, Ordering::SeqCst);
    let record =
        attach::subscription(state.subscriber, Some(ignore_delivery), ptr::null_mut(), None);
    let mut id = 0;
    // SAFETY: the other `WaitSet` is live; the releasing registration still holds the subscriber.
    let status = unsafe { hypha_waitset_attach(state.other, &raw const record, &raw mut id) };
    state.status.store(status as i32, Ordering::SeqCst);
    if status == HyphaStatus::Ok {
        // SAFETY: as above; `id` is the registration just attached.
        let detached = unsafe { hypha_waitset_detach(state.other, id) };
        assert_eq!(detached, HyphaStatus::Ok);
    }
}

#[test]
fn a_subscription_release_at_the_waitset_destroy_runs_after_it_stopped_dispatching() {
    let node = create_node(&unique("release_at_destroy"));
    let other_node = create_node(&unique("release_at_destroy_other"));
    // SAFETY: both nodes are live nodes just created.
    let (waitset, other) = unsafe { (create_waitset(node), create_waitset(other_node)) };
    let (publisher, subscriber) = pubsub(node, "release_at_destroy");
    let state = AttachElsewhere {
        other,
        subscriber,
        releases: AtomicU64::new(0),
        status: AtomicI32::new(i32::MIN),
    };
    let context = std::ptr::from_ref(&state).cast_mut().cast::<c_void>();
    // SAFETY: the handles are live; `state` outlives both nodes.
    let _id =
        unsafe { attach_with(waitset, subscriber, context, Some(attach_elsewhere_on_release)) };

    // The caller's endpoint handles go first, as the node's destruction requires; the
    // registration still shares the subscriber.
    // SAFETY: every handle is live and destroyed once.
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    assert_eq!(state.releases.load(Ordering::SeqCst), 1, "the destroy releases once");
    assert_eq!(
        state.status.load(Ordering::SeqCst),
        HyphaStatus::Ok as i32,
        "inside the release the destroyed WaitSet no longer dispatches the subscriber"
    );
    // SAFETY: the second node is live and destroyed once.
    unsafe { hypha_node_destroy(other_node) };
}

const SAMPLE_TYPE: &str = "std_msgs/UInt8";
const SAMPLE_MD5: &str = "7c8164229e7d2c17eb95e9231617fdee";

/// The registration a subscription callback detaches, and the `WaitSet` it detaches from.
struct DetachSelf {
    waitset: *mut HyphaWaitSet,
    attachment: AtomicU64,
    detach_status: AtomicI32,
    release: AttachElsewhere,
}

/// Detaches its own registration on the first sample, and stops the run.
extern "C" fn detach_self(
    _delivery: *const HyphaDelivery,
    context: *mut c_void,
) -> HyphaCallbackResult {
    // SAFETY: the context is the test's `DetachSelf`, alive until both nodes go.
    let state = unsafe { &*context.cast::<DetachSelf>() };
    let attachment = state.attachment.load(Ordering::SeqCst);
    // SAFETY: the `WaitSet` is live and runs on this thread.
    let status = unsafe { hypha_waitset_detach(state.waitset, attachment) };
    state.detach_status.store(status as i32, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_STOP, status: 0 }
}

/// The release of a `DetachSelf` context: what `attach_elsewhere_on_release` does, for
/// its `release` field.
extern "C" fn detach_self_release(context: *mut c_void) {
    // SAFETY: the context is the test's `DetachSelf`, alive until both nodes go.
    let state = unsafe { &*context.cast::<DetachSelf>() };
    let release = std::ptr::from_ref(&state.release).cast_mut().cast::<c_void>();
    attach_elsewhere_on_release(release);
}

#[test]
fn a_subscription_release_of_a_detach_in_its_callback_runs_after_it_stopped_dispatching() {
    let node = create_node(&unique("release_deferred"));
    let other_node = create_node(&unique("release_deferred_other"));
    // SAFETY: both nodes are live nodes just created.
    let (waitset, other) = unsafe { (create_waitset(node), create_waitset(other_node)) };
    let topic = format!("/{}", unique("release_deferred"));
    // SAFETY: `node` is live; the helpers open one endpoint each.
    let publisher =
        unsafe { common::replay::open_publisher(node, &topic, SAMPLE_TYPE, SAMPLE_MD5, "") };
    // SAFETY: as above.
    let subscriber = unsafe { common::replay::open_subscriber(node, &topic, SAMPLE_MD5, 8) };
    let state = DetachSelf {
        waitset,
        attachment: AtomicU64::new(0),
        detach_status: AtomicI32::new(i32::MIN),
        release: AttachElsewhere {
            other,
            subscriber,
            releases: AtomicU64::new(0),
            status: AtomicI32::new(i32::MIN),
        },
    };
    let context = std::ptr::from_ref(&state).cast_mut().cast::<c_void>();
    let record =
        attach::subscription(subscriber, Some(detach_self), context, Some(detach_self_release));
    let mut id = 0;
    // SAFETY: the handles are live; `record` and `id` are locals; `state` outlives both nodes.
    let attached = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut id) };
    assert_eq!(attached, HyphaStatus::Ok);
    state.attachment.store(id, Ordering::SeqCst);
    // SAFETY: `publisher` is live.
    unsafe { common::replay::send_stamped(publisher, 1, 1) };

    let mut report =
        HyphaWaitSetRunReport { reason: 99, status: 0, attachment: 0, slow_callbacks: 0 };
    // SAFETY: `waitset` is live; `report` is a local.
    let run = unsafe { hypha_waitset_run(waitset, 2_000_000_000, &raw mut report) };
    assert_eq!(run, HyphaStatus::Ok);
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED, "the callback got the sample: {report:?}");
    assert_eq!(state.detach_status.load(Ordering::SeqCst), HyphaStatus::Ok as i32);
    assert_eq!(state.release.releases.load(Ordering::SeqCst), 1, "released when the run returned");
    assert_eq!(
        state.release.status.load(Ordering::SeqCst),
        HyphaStatus::Ok as i32,
        "inside the release the detached registration no longer dispatches the subscriber"
    );

    // SAFETY: every handle is live and destroyed once, the nodes last.
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
        hypha_node_destroy(other_node);
    }
    assert_eq!(state.release.releases.load(Ordering::SeqCst), 1, "not released again");
}
