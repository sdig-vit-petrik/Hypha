//! A `WaitSet` is claimed by the thread inside a call on it, for that call (#83): a run,
//! or any other call that names it, started meanwhile on another thread is refused with
//! `InvalidState` and changes nothing. Before the claim that race was undetected (AoU-22).
//!
//! Thread A detaches a timer; the timer's context release runs inside that detach, on A,
//! and there another thread B tries a run and a timer read on the same `WaitSet`. No step
//! depends on timing: B runs to completion while A waits for it inside the call.
#![allow(clippy::expect_used, reason = "a failed test setup should stop the test with its message")]

mod common;
use common::attach;
use common::{create_node, unique};

use std::ffi::c_void;
use std::sync::Mutex;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

/// What thread B saw from inside thread A's detach: the run's status and the timer read's.
static SEEN_FROM_B: Mutex<Option<(HyphaStatus, HyphaStatus)>> = Mutex::new(None);
/// The `WaitSet` and the timer B uses, as addresses.
static TARGET: Mutex<Option<(usize, HyphaAttachmentId)>> = Mutex::new(None);

extern "C" fn tick(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// Runs inside `hypha_waitset_detach` on thread A: thread B tries the `WaitSet` meanwhile.
extern "C" fn release_while_b_tries(_context: *mut c_void) {
    let (waitset, timer) = TARGET.lock().expect("the target lock").expect("the target is set");
    let seen = std::thread::spawn(move || {
        let waitset = waitset as *mut HyphaWaitSet;
        let mut report =
            HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
        // SAFETY: the node, and its `WaitSet`, outlive this thread, which A joins below.
        let run = unsafe { hypha_waitset_run(waitset, 1_000_000, &raw mut report) };
        // SAFETY: every field of the plain C record accepts all-zero bits.
        let mut state: HyphaTimerState = unsafe { std::mem::zeroed() };
        // SAFETY: as above; `state` is a local.
        let read = unsafe { hypha_waitset_timer_state(waitset, timer, &raw mut state) };
        (run, read)
    })
    .join()
    .expect("thread B ends");
    *SEEN_FROM_B.lock().expect("the result lock") = Some(seen);
}

/// Attaches a timer of `waitset` whose release is `release`.
///
/// # Safety
/// `waitset` must be live.
unsafe fn attach_timer(
    waitset: *mut HyphaWaitSet,
    release: HyphaContextRelease,
) -> HyphaAttachmentId {
    let record = attach::timer(1_000_000_000, Some(tick), std::ptr::null_mut(), release);
    let mut id = 0;
    // SAFETY: `waitset` is live by the caller; `record` and `id` are locals.
    let status = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut id) };
    assert_eq!(status, HyphaStatus::Ok);
    id
}

#[test]
fn a_run_or_a_call_from_another_thread_inside_a_call_is_refused() {
    let node = create_node(&unique("waitset_claim"));
    let mut waitset = std::ptr::null_mut();
    // SAFETY: `node` is live; `waitset` is a local.
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    // SAFETY: `waitset` is live.
    let detached = unsafe { attach_timer(waitset, Some(release_while_b_tries)) };
    // SAFETY: as above.
    let kept = unsafe { attach_timer(waitset, None) };
    *TARGET.lock().expect("the target lock") = Some((waitset as usize, kept));

    // SAFETY: `waitset` is live; the release runs inside this call, on this thread.
    let status = unsafe { hypha_waitset_detach(waitset, detached) };
    assert_eq!(status, HyphaStatus::Ok);
    let seen = SEEN_FROM_B.lock().expect("the result lock").take();
    assert_eq!(
        seen,
        Some((HyphaStatus::InvalidState, HyphaStatus::InvalidState)),
        "a run and a timer read beside a call on another thread are refused"
    );

    // Once the call returned, the claim is released: another thread runs and reads.
    let handle = waitset as usize;
    let after = std::thread::spawn(move || {
        let waitset = handle as *mut HyphaWaitSet;
        let mut report =
            HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
        // SAFETY: the node outlives this thread, which the test joins.
        let run = unsafe { hypha_waitset_run(waitset, 1_000_000, &raw mut report) };
        // SAFETY: every field of the plain C record accepts all-zero bits.
        let mut state: HyphaTimerState = unsafe { std::mem::zeroed() };
        // SAFETY: as above; `state` is a local.
        let read = unsafe { hypha_waitset_timer_state(waitset, kept, &raw mut state) };
        (run, read)
    })
    .join()
    .expect("the later thread ends");
    assert_eq!(after, (HyphaStatus::Ok, HyphaStatus::Ok));
    // SAFETY: `node` is live and not used afterwards.
    unsafe { hypha_node_destroy(node) };
}
