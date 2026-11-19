//! The C ABI wrappers of `hypha_node::waitset::run`.

use super::access::{shared_of, waitset_here};
use hypha_node::safe::{HyphaWaitSet, HyphaWaitSetRunReport, waitset_run};
use hypha_node::status::{Error, HyphaStatus};
use std::time::Duration;

/// Run on the calling thread until a stop or a failure, or for at most `limit_nanos` of
/// host time when it is nonzero (reason `HYPHA_RUN_REASON_DEADLINE`, status `Ok`). A
/// second run of the `WaitSet` returns `InvalidState` and fills no report. A completed
/// invocation returns [`HyphaStatus::Ok`] and fills the report, even when the run itself
/// failed; a nonzero report status is a run failure.
///
/// # Safety
/// `waitset` is live. `out_report` points to one writable [`HyphaWaitSetRunReport`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_waitset_run(
    waitset: *mut HyphaWaitSet,
    limit_nanos: u64,
    out_report: *mut HyphaWaitSetRunReport,
) -> HyphaStatus {
    if out_report.is_null() {
        return HyphaStatus::InvalidArgument;
    }
    // SAFETY: the `WaitSet` is null or live by the caller's contract.
    let set = match unsafe { waitset_here(waitset) } {
        Ok(set) => set,
        Err(status) => return status,
    };
    let limit = (limit_nanos != 0).then(|| Duration::from_nanos(limit_nanos));
    match waitset_run(set, limit) {
        Ok(report) => {
            unsafe { out_report.write(report) };
            HyphaStatus::Ok
        }
        Err(error) => HyphaStatus::from(error),
    }
}

/// Ask a `WaitSet` to stop. Idempotent. With no run in progress it returns
/// `InvalidState` and still records the stop, so the next run returns at once. Null:
/// `InvalidArgument`.
///
/// # Safety
/// `waitset` must be a live handle. Any thread may call it on a running `WaitSet`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_waitset_request_stop(waitset: *mut HyphaWaitSet) -> HyphaStatus {
    // SAFETY: the `WaitSet` is live by the caller's contract.
    let Some(shared) = (unsafe { shared_of(waitset) }) else {
        return HyphaStatus::InvalidArgument;
    };
    stop_status(shared.request_stop())
}

/// Ask the run of a `WaitSet` to end as failed with [`HyphaStatus::ApplicationFailed`].
///
/// Requests the stop as [`hypha_waitset_request_stop`] does, with its return values.
/// The run that ends while it is recorded reports `HYPHA_RUN_REASON_RUNTIME_FAILED` with
/// this status and clears it; recorded before a run, that run fails at once. A cycle
/// step that returns while it is recorded sends none of its staged messages; outputs
/// sent before a request from another thread stay sent.
/// # Safety
/// `waitset` must be a live handle or null. Any thread may call it on a running
/// `WaitSet`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hypha_waitset_request_failure(waitset: *mut HyphaWaitSet) -> HyphaStatus {
    // SAFETY: the `WaitSet` is live by the caller's contract.
    let Some(shared) = (unsafe { shared_of(waitset) }) else {
        return HyphaStatus::InvalidArgument;
    };
    stop_status(shared.request_failure())
}

fn stop_status(requested: Result<bool, Error>) -> HyphaStatus {
    match requested {
        Ok(true) => HyphaStatus::Ok,
        Ok(false) => HyphaStatus::InvalidState,
        Err(error) => HyphaStatus::from(error),
    }
}
