//! The C ABI wrappers of `hypha_node::waitset::timer_control`.
//!
//! They apply to a plain timer (`HYPHA_ATTACH_TIMER`) only: a cycle's own timer, a
//! subscription or an unknown id is `InvalidArgument`. A null `waitset` is
//! `InvalidArgument`. Between runs any thread may call them, one at a time; during a run
//! only the run's own thread (a callback), and another thread gets `InvalidState` with
//! nothing changed.

use super::access::waitset_here;
use hypha_node::HyphaAttachmentId;
use hypha_node::safe::{
    HyphaTimerState, HyphaWaitSet, waitset_timer_cancel, waitset_timer_reset, waitset_timer_state,
};
use hypha_node::status::{HyphaStatus, status_of};

/// Cancel a plain timer: it calls nothing until `hypha_waitset_timer_reset`, counts no
/// missed period and holds no wake-up of the run. Idempotent. From a callback it also
/// holds back a call of the timer still due later in the same dispatch turn. A canceled
/// timer on simulated time still follows its clock: time going back ends the run with
/// `ClockRewound`, as for any timer, and so does a reset to a time earlier than it saw.
///
/// `InvalidArgument`: null `waitset`, or a `timer` that names no plain timer of it (a
/// cycle's timer, a subscription, an unknown id). `InvalidState`: another thread than
/// the run's during a run.
///
/// # Safety
/// `waitset` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_waitset_timer_cancel(
    waitset: *mut HyphaWaitSet,
    timer: HyphaAttachmentId,
) -> HyphaStatus {
    // SAFETY: the `WaitSet` is null or live by the caller's contract.
    let set = match unsafe { waitset_here(waitset) } {
        Ok(set) => set,
        Err(status) => return status,
    };
    status_of(waitset_timer_cancel(set, timer))
}

/// Reset a plain timer: its next call comes one period after now on the clock it was
/// created on (simulated framework time under simulated time, else the host monotonic
/// scheduling clock), and a cancel ends. The time before the reset counts as missed in
/// no call. From a callback it takes effect before the run's next wait and holds back a
/// call of the timer still due later in the same dispatch turn.
///
/// `InvalidArgument` and `InvalidState` as `hypha_waitset_timer_cancel`.
///
/// # Safety
/// `waitset` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_waitset_timer_reset(
    waitset: *mut HyphaWaitSet,
    timer: HyphaAttachmentId,
) -> HyphaStatus {
    // SAFETY: the `WaitSet` is null or live by the caller's contract.
    let set = match unsafe { waitset_here(waitset) } {
        Ok(set) => set,
        Err(status) => return status,
    };
    status_of(waitset_timer_reset(set, timer))
}

/// Write the state of a plain timer to `out`, read at the call (see `HyphaTimerState`).
/// It does not wait for the run: a callback may read it.
///
/// `InvalidArgument`: null `waitset` or `out`, or a `timer` that names no plain timer of
/// it. `InvalidState`: another thread than the run's during a run. Only `Ok` writes
/// `out`.
///
/// # Safety
/// `waitset` must be null or a live handle; `out` null or writable for one
/// `HyphaTimerState`, which the caller owns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_waitset_timer_state(
    waitset: *const HyphaWaitSet,
    timer: HyphaAttachmentId,
    out: *mut HyphaTimerState,
) -> HyphaStatus {
    if out.is_null() {
        return HyphaStatus::InvalidArgument;
    }
    // SAFETY: the `WaitSet` is null or live by the caller's contract.
    let set = match unsafe { waitset_here(waitset) } {
        Ok(set) => set,
        Err(status) => return status,
    };
    match waitset_timer_state(set, timer) {
        Ok(state) => {
            // SAFETY: `out` is non-null by the check above and writable by the contract.
            unsafe { out.write(state) };
            HyphaStatus::Ok
        }
        Err(error) => HyphaStatus::from(error),
    }
}
