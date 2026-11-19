//! A callback may detach a registration of its own `WaitSet` during the run.
//! Each test runs in its own process under nextest.
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use hypha_ffi::waitset::{HYPHA_ACTION_CONTINUE, HyphaCallbackResult};
use hypha_ffi::*;

use crate::common::{attach, unique};

/// The state of a timer that detaches itself on its first call.
struct SelfDetaching {
    waitset: *mut HyphaWaitSet,
    attachment: AtomicU64,
    calls: AtomicU32,
    detach_status: AtomicU32,
    releases: AtomicU32,
    calls_at_release: AtomicU32,
}

extern "C" fn detach_on_first_call(_missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    let state = unsafe { &*context.cast::<SelfDetaching>() };
    let calls = state.calls.fetch_add(1, Ordering::SeqCst);
    if calls == 0 {
        let attachment = state.attachment.load(Ordering::SeqCst);
        let status = unsafe { hypha_waitset_detach(state.waitset, attachment) };
        state.detach_status.store(status as u32, Ordering::SeqCst);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn count_release(context: *mut c_void) {
    let state = unsafe { &*context.cast::<SelfDetaching>() };
    state.releases.fetch_add(1, Ordering::SeqCst);
    state.calls_at_release.store(state.calls.load(Ordering::SeqCst), Ordering::SeqCst);
}

fn run(waitset: *mut HyphaWaitSet, nanos: u64) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, nanos, &raw mut report) }, HyphaStatus::Ok);
    assert_eq!(report.status, HyphaStatus::Ok as i32, "{report:?}");
}

#[test]
fn a_timer_that_detaches_itself_stops_at_once_and_is_released_once_after_the_run() {
    let node = common::create_node(&unique("detach_cb"));
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let state = SelfDetaching {
        waitset,
        attachment: AtomicU64::new(0),
        calls: AtomicU32::new(0),
        detach_status: AtomicU32::new(u32::MAX),
        releases: AtomicU32::new(0),
        calls_at_release: AtomicU32::new(0),
    };
    let context = ptr::from_ref(&state).cast_mut().cast::<c_void>();
    let record = attach::timer(5_000_000, Some(detach_on_first_call), context, Some(count_release));
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    state.attachment.store(attachment, Ordering::SeqCst);
    run(waitset, 60_000_000);
    assert_eq!(state.detach_status.load(Ordering::SeqCst), HyphaStatus::Ok as u32);
    assert_eq!(state.calls.load(Ordering::SeqCst), 1, "no call after the detach");
    assert_eq!(state.releases.load(Ordering::SeqCst), 1, "released once, when the run returned");
    // A second run finds no registration and releases nothing again.
    run(waitset, 20_000_000);
    assert_eq!(state.releases.load(Ordering::SeqCst), 1);
    let again = unsafe { hypha_waitset_detach(waitset, attachment) };
    assert_eq!(again, HyphaStatus::InvalidArgument, "the registration is gone");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_hundred_and_fifty_runs_of_attach_and_self_detach_never_reach_the_limit() {
    let node = common::create_node(&unique("detach_many"));
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    for _ in 0..150 {
        let state = SelfDetaching {
            waitset,
            attachment: AtomicU64::new(0),
            calls: AtomicU32::new(0),
            detach_status: AtomicU32::new(u32::MAX),
            releases: AtomicU32::new(0),
            calls_at_release: AtomicU32::new(0),
        };
        let context = ptr::from_ref(&state).cast_mut().cast::<c_void>();
        let record =
            attach::timer(1_000_000, Some(detach_on_first_call), context, Some(count_release));
        let mut attachment = 0;
        assert_eq!(
            unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
            HyphaStatus::Ok,
            "the registration limit counts live registrations only"
        );
        state.attachment.store(attachment, Ordering::SeqCst);
        run(waitset, 5_000_000);
        assert_eq!(state.releases.load(Ordering::SeqCst), 1);
    }
    unsafe { hypha_node_destroy(node) };
}

/// Detaches from another thread while the `WaitSet` runs.
struct OtherThread {
    waitset: usize,
    attachment: u64,
}

extern "C" fn tick(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_detach_from_another_thread_during_a_run_is_refused() {
    let node = common::create_node(&unique("detach_thread"));
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let record = attach::timer(5_000_000, Some(tick), ptr::null_mut(), None);
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    let other = OtherThread { waitset: waitset as usize, attachment };
    let runner = std::thread::spawn(move || {
        let waitset = other.waitset as *mut HyphaWaitSet;
        let mut report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        unsafe { hypha_waitset_run(waitset, 200_000_000, &raw mut report) }
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    let status = unsafe { hypha_waitset_detach(waitset, other.attachment) };
    assert_eq!(status, HyphaStatus::InvalidState);
    assert_eq!(runner.join().expect("the run thread"), HyphaStatus::Ok);
    unsafe { hypha_node_destroy(node) };
}

const SAMPLE_TYPE: &str = "std_msgs/UInt8";
const SAMPLE_MD5: &str = "7c8164229e7d2c17eb95e9231617fdee";

/// A subscription that detaches itself on its first sample, then optionally blocks.
struct SelfDetachingSubscription {
    waitset: *mut HyphaWaitSet,
    attachment: AtomicU64,
    calls: AtomicU32,
    releases: AtomicU32,
    block_after_detach: std::time::Duration,
}

extern "C" fn detach_on_first_sample(
    _delivery: *const HyphaDelivery,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let state = unsafe { &*context.cast::<SelfDetachingSubscription>() };
    let calls = state.calls.fetch_add(1, Ordering::SeqCst);
    if calls == 0 {
        let attachment = state.attachment.load(Ordering::SeqCst);
        let status = unsafe { hypha_waitset_detach(state.waitset, attachment) };
        assert_eq!(status, HyphaStatus::Ok);
        std::thread::sleep(state.block_after_detach);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn count_subscription_release(context: *mut c_void) {
    let state = unsafe { &*context.cast::<SelfDetachingSubscription>() };
    state.releases.fetch_add(1, Ordering::SeqCst);
}

/// A node, its `WaitSet`, a publisher with three queued samples and a self-detaching
/// subscription on them.
fn self_detaching_subscription(
    label: &str,
    block: std::time::Duration,
) -> (*mut HyphaNode, *mut HyphaWaitSet, Box<SelfDetachingSubscription>) {
    let node = common::create_node(&unique(label));
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let topic = format!("/{}", unique(label));
    let publisher =
        unsafe { common::replay::open_publisher(node, &topic, SAMPLE_TYPE, SAMPLE_MD5, "") };
    let subscriber = unsafe { common::replay::open_subscriber(node, &topic, SAMPLE_MD5, 8) };
    let state = Box::new(SelfDetachingSubscription {
        waitset,
        attachment: AtomicU64::new(0),
        calls: AtomicU32::new(0),
        releases: AtomicU32::new(0),
        block_after_detach: block,
    });
    let context = ptr::from_ref(&*state).cast_mut().cast::<c_void>();
    let record = attach::subscription(
        subscriber,
        Some(detach_on_first_sample),
        context,
        Some(count_subscription_release),
    );
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    state.attachment.store(attachment, Ordering::SeqCst);
    for value in 1..=3_u8 {
        unsafe { common::replay::send_stamped(publisher, value, i64::from(value)) };
    }
    (node, waitset, state)
}

#[test]
fn a_subscription_that_detaches_itself_gets_no_queued_sample_after_it() {
    let (node, waitset, state) =
        self_detaching_subscription("detach_sub", std::time::Duration::ZERO);
    run(waitset, 50_000_000);
    assert_eq!(state.calls.load(Ordering::SeqCst), 1, "the two queued samples are not delivered");
    assert_eq!(state.releases.load(Ordering::SeqCst), 1);
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_refused_second_run_leaves_the_running_threads_detach_alone() {
    let (node, waitset, state) =
        self_detaching_subscription("detach_race", std::time::Duration::from_millis(150));
    let address = waitset as usize;
    let runner = std::thread::spawn(move || {
        let waitset = address as *mut HyphaWaitSet;
        let mut report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        unsafe { hypha_waitset_run(waitset, 300_000_000, &raw mut report) }
    });
    // While the callback blocks after its detach, a second run from this thread is refused.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    let refused = unsafe { hypha_waitset_run(waitset, 10_000_000, &raw mut report) };
    assert_eq!(refused, HyphaStatus::InvalidState);
    assert_eq!(runner.join().expect("the run thread"), HyphaStatus::Ok);
    assert_eq!(state.calls.load(Ordering::SeqCst), 1, "the detached subscription stays dead");
    assert_eq!(state.releases.load(Ordering::SeqCst), 1, "released once, by the running thread");
    unsafe { hypha_node_destroy(node) };
}
