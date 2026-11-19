//! A `WaitSet` owns its cycles; `hypha_cycle_destroy` releases one earlier.
#![allow(clippy::expect_used)]

mod common;
use crate::common::{c_text, create_node, unique};

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use hypha_ffi::*;

static RELEASES: AtomicU32 = AtomicU32::new(0);

extern "C" fn count_release(_context: *mut c_void) {
    RELEASES.fetch_add(1, Ordering::SeqCst);
}

extern "C" fn step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> waitset::HyphaCallbackResult {
    waitset::HyphaCallbackResult { action: waitset::HYPHA_ACTION_CONTINUE, status: 0 }
}

unsafe fn default_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    waitset
}

unsafe fn create_cycle(waitset: *mut HyphaWaitSet, name: &str) -> (HyphaStatus, *mut HyphaCycle) {
    let name = c_text(name);
    let mut cycle = ptr::null_mut();
    let status = unsafe {
        hypha_cycle_create(
            waitset,
            name.as_ptr(),
            name.as_bytes().len(),
            10_000_000,
            Some(step),
            ptr::null_mut(),
            Some(count_release),
            &raw mut cycle,
        )
    };
    (status, cycle)
}

#[test]
fn the_node_destroys_the_cycles_of_its_waitset_and_releases_each_context_once() {
    let node = create_node(&unique("cycle_owner"));
    let waitset = unsafe { default_waitset(node) };
    let (status, committed) = unsafe { create_cycle(waitset, "committed") };
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(committed) }, HyphaStatus::Ok);
    let (status, uncommitted) = unsafe { create_cycle(waitset, "uncommitted") };
    assert_eq!(status, HyphaStatus::Ok);
    assert!(!uncommitted.is_null());

    // A second cycle of the same name is refused, and the first one stays.
    let (duplicate, refused) = unsafe { create_cycle(waitset, "committed") };
    assert_eq!(duplicate, HyphaStatus::InvalidArgument);
    assert!(refused.is_null());

    let before = RELEASES.load(Ordering::SeqCst);
    unsafe { hypha_node_destroy(node) };
    assert_eq!(RELEASES.load(Ordering::SeqCst), before.wrapping_add(2));
}

#[test]
fn an_early_destroy_releases_one_cycle_and_the_waitset_forgets_it() {
    let node = create_node(&unique("cycle_early"));
    let waitset = unsafe { default_waitset(node) };
    let (status, cycle) = unsafe { create_cycle(waitset, "early") };
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    let before = RELEASES.load(Ordering::SeqCst);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(RELEASES.load(Ordering::SeqCst), before.wrapping_add(1));

    // The name is free again, and the node releases the new cycle.
    let (status, again) = unsafe { create_cycle(waitset, "early") };
    assert_eq!(status, HyphaStatus::Ok);
    assert!(!again.is_null());
    unsafe { hypha_node_destroy(node) };
    assert_eq!(RELEASES.load(Ordering::SeqCst), before.wrapping_add(2));
}

extern "C" fn count_own_release(context: *mut c_void) {
    // SAFETY: each context of this release is an `AtomicU32` the test keeps alive.
    let counter = unsafe { &*context.cast::<AtomicU32>() };
    counter.fetch_add(1, Ordering::SeqCst);
}

/// # Safety
/// `waitset` must be live, and `releases` must outlive the cycle.
unsafe fn create_counted(
    waitset: *mut HyphaWaitSet,
    name: &str,
    releases: &AtomicU32,
) -> (HyphaStatus, *mut HyphaCycle) {
    let name = c_text(name);
    let mut cycle = ptr::null_mut();
    let context = std::ptr::from_ref(releases).cast_mut().cast::<c_void>();
    // SAFETY: `waitset` is live by the caller; `cycle` is a local.
    let status = unsafe {
        hypha_cycle_create(
            waitset,
            name.as_ptr(),
            name.as_bytes().len(),
            10_000_000,
            Some(step),
            context,
            Some(count_own_release),
            &raw mut cycle,
        )
    };
    (status, cycle)
}

#[test]
fn a_refused_cycle_create_leaves_the_context_with_the_caller() {
    let node = create_node(&unique("cycle_refused_context"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { default_waitset(node) };
    let kept = AtomicU32::new(0);
    // SAFETY: `waitset` is live and `kept` outlives the node.
    let (status, _cycle) = unsafe { create_counted(waitset, "taken", &kept) };
    assert_eq!(status, HyphaStatus::Ok);

    let refused = AtomicU32::new(0);
    // SAFETY: as above.
    let (duplicate, cycle) = unsafe { create_counted(waitset, "taken", &refused) };
    assert_eq!(duplicate, HyphaStatus::InvalidArgument);
    assert!(cycle.is_null(), "a refused create writes no handle");
    // SAFETY: as above.
    let (unnamed, cycle) = unsafe { create_counted(waitset, "", &refused) };
    assert_eq!(unnamed, HyphaStatus::InvalidArgument);
    assert!(cycle.is_null(), "a refused create writes no handle");
    assert_eq!(refused.load(Ordering::SeqCst), 0, "a refused create must not release");

    // SAFETY: `node` is live and destroyed once.
    unsafe { hypha_node_destroy(node) };
    assert_eq!(kept.load(Ordering::SeqCst), 1, "the accepted cycle is released once");
    assert_eq!(refused.load(Ordering::SeqCst), 0, "nothing ever releases a refused context");
}

extern "C" fn release_reuses_the_name(context: *mut c_void) {
    // The name is free and the waitset is not borrowed: a new cycle of the same
    // name is accepted from inside the release.
    let waitset = context.cast::<HyphaWaitSet>();
    let (status, again) = unsafe { create_cycle(waitset, "reentrant") };
    assert_eq!(status, HyphaStatus::Ok);
    assert!(!again.is_null());
    RELEASES.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn a_cycle_release_callback_sees_the_waitset_unlocked_and_the_cycle_gone() {
    let node = create_node(&unique("cycle_reentrant_release"));
    let waitset = unsafe { default_waitset(node) };
    let name = c_text("reentrant");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000,
                Some(step),
                waitset.cast(),
                Some(release_reuses_the_name),
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let before = RELEASES.load(Ordering::SeqCst);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(RELEASES.load(Ordering::SeqCst), before.wrapping_add(1));
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_cycle_never_committed_is_released_once_with_its_waitset() {
    let node = create_node(&unique("cycle_never_committed"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { default_waitset(node) };
    let releases = AtomicU32::new(0);
    // SAFETY: `waitset` is live and `releases` outlives the node.
    let (status, cycle) = unsafe { create_counted(waitset, "never_committed", &releases) };
    assert_eq!(status, HyphaStatus::Ok);
    assert!(!cycle.is_null());
    assert_eq!(releases.load(Ordering::SeqCst), 0, "a live cycle keeps its context");

    // SAFETY: `node` is live and destroyed once; it destroys its `WaitSet` first.
    unsafe { hypha_node_destroy(node) };
    assert_eq!(releases.load(Ordering::SeqCst), 1, "the WaitSet releases the context once");
}

extern "C" fn no_tick(_missed: u64, _context: *mut c_void) -> waitset::HyphaCallbackResult {
    waitset::HyphaCallbackResult { action: waitset::HYPHA_ACTION_CONTINUE, status: 0 }
}

/// # Safety
/// `waitset` must be live and not running.
unsafe fn spare_registrations(waitset: *mut HyphaWaitSet) -> usize {
    let record = common::attach::timer(1_000_000_000, Some(no_tick), ptr::null_mut(), None);
    let mut accepted = Vec::new();
    loop {
        let mut id = 0;
        // SAFETY: `waitset` is live by the caller; `record` and `id` are locals.
        let status =
            unsafe { waitset::hypha_waitset_attach(waitset, &raw const record, &raw mut id) };
        if status == HyphaStatus::ResourceLimit {
            break;
        }
        assert_eq!(status, HyphaStatus::Ok, "an attach below the limit is accepted");
        accepted.push(id);
    }
    for id in &accepted {
        // SAFETY: as above; `id` is a registration just attached.
        assert_eq!(unsafe { waitset::hypha_waitset_detach(waitset, *id) }, HyphaStatus::Ok);
    }
    accepted.len()
}

#[test]
fn a_destroyed_rate_cycle_released_its_context_once_and_left_no_registration() {
    let node = create_node(&unique("cycle_destroyed_rate"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { default_waitset(node) };
    // SAFETY: `waitset` is live and not running.
    let spare = unsafe { spare_registrations(waitset) };
    let releases = AtomicU32::new(0);
    // SAFETY: `waitset` is live and `releases` outlives the node.
    let (status, cycle) = unsafe { create_counted(waitset, "destroyed_rate", &releases) };
    assert_eq!(status, HyphaStatus::Ok);
    // SAFETY: `cycle` is live.
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    // SAFETY: as above.
    let with_timer = unsafe { spare_registrations(waitset) };
    assert_eq!(with_timer + 1, spare, "the timer is registered");

    // SAFETY: `cycle` is live and destroyed once; the handle is not used afterwards.
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(releases.load(Ordering::SeqCst), 1);
    // SAFETY: as above.
    assert_eq!(unsafe { spare_registrations(waitset) }, spare, "the timer is gone");

    // SAFETY: `node` is live and destroyed once.
    unsafe { hypha_node_destroy(node) };
    assert_eq!(releases.load(Ordering::SeqCst), 1, "a destroyed cycle is not released again");
}

/// What a timer callback tries inside the run, and what it got.
struct CreateInsideRun {
    waitset: *mut HyphaWaitSet,
    releases: AtomicU32,
    status: std::sync::atomic::AtomicI32,
    handle_written: std::sync::atomic::AtomicBool,
}

/// Inside the run: create a counted cycle, record the status, and stop the run.
extern "C" fn create_inside_the_run(
    _missed: u64,
    context: *mut c_void,
) -> waitset::HyphaCallbackResult {
    // SAFETY: the context is the test's `CreateInsideRun`, alive until the node goes.
    let state = unsafe { &*context.cast::<CreateInsideRun>() };
    // SAFETY: the `WaitSet` is live; the counter outlives the node.
    let (status, cycle) = unsafe { create_counted(state.waitset, "inside_run", &state.releases) };
    state.status.store(status as i32, Ordering::SeqCst);
    state.handle_written.store(!cycle.is_null(), Ordering::SeqCst);
    waitset::HyphaCallbackResult { action: waitset::HYPHA_ACTION_STOP, status: 0 }
}

#[test]
fn a_cycle_create_refused_inside_a_run_leaves_the_context_to_the_caller() {
    let node = create_node(&unique("cycle_refused_running"));
    // SAFETY: `node` is the live node just created.
    let waitset = unsafe { default_waitset(node) };
    let state = CreateInsideRun {
        waitset,
        releases: AtomicU32::new(0),
        status: std::sync::atomic::AtomicI32::new(i32::MIN),
        handle_written: std::sync::atomic::AtomicBool::new(true),
    };
    let context = std::ptr::from_ref(&state).cast_mut().cast::<c_void>();
    let record = common::attach::timer(1_000_000, Some(create_inside_the_run), context, None);
    let mut id = 0;
    // SAFETY: `waitset` is live; `record` and `id` are locals; `state` outlives the node.
    let attached =
        unsafe { waitset::hypha_waitset_attach(waitset, &raw const record, &raw mut id) };
    assert_eq!(attached, HyphaStatus::Ok);

    let mut report =
        waitset::HyphaWaitSetRunReport { reason: 99, status: 0, attachment: 0, slow_callbacks: 0 };
    // SAFETY: `waitset` is live; `report` is a local.
    let run = unsafe { waitset::hypha_waitset_run(waitset, 2_000_000_000, &raw mut report) };
    assert_eq!(run, HyphaStatus::Ok);
    assert_eq!(report.reason, waitset::HYPHA_RUN_REASON_STOPPED, "the callback ran");
    assert_eq!(state.status.load(Ordering::SeqCst), HyphaStatus::InvalidState as i32);
    assert!(!state.handle_written.load(Ordering::SeqCst), "a refused create writes no handle");
    assert_eq!(state.releases.load(Ordering::SeqCst), 0, "a refused create must not release");

    // The caller still owns the context: the same create is accepted after the run, and
    // the context is released once, with the `WaitSet`.
    // SAFETY: as above.
    let (status, cycle) = unsafe { create_counted(waitset, "inside_run", &state.releases) };
    assert_eq!(status, HyphaStatus::Ok);
    assert!(!cycle.is_null());
    // SAFETY: `node` is live and destroyed once.
    unsafe { hypha_node_destroy(node) };
    assert_eq!(state.releases.load(Ordering::SeqCst), 1);
}
