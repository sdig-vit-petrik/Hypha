//! `WaitSet` run ownership and C ABI implementation.

//!
//! `hypha_waitset_request_stop` is idempotent and always records the stop, even before
//! the run starts; the run consumes it, so a stop never carries into a later run. It
//! returns [`HyphaStatus::InvalidState`] when no run was in progress, which is not a refusal.
//!
//! `hypha_waitset_request_failure` records the failure, then requests the stop. The run that
//! ends while it is recorded reports `HYPHA_RUN_REASON_RUNTIME_FAILED` with status
//! `ApplicationFailed`, and clears it. A cycle step that returns while it is recorded sends
//! none of its staged messages.

use hypha_core::port::dispatch::RunReport;
use hypha_runtime::RunDriver;

use super::{
    AttachmentId, CallbackDispatcher, Duration, Error, HYPHA_RUN_REASON_DEADLINE,
    HYPHA_RUN_REASON_RUNTIME_FAILED, HYPHA_RUN_REASON_SETUP_FAILED, HYPHA_RUN_REASON_STOPPED,
    HyphaStatus, HyphaWaitSet, HyphaWaitSetRunReport, IN_DISPATCH, StopReason, in_dispatch,
};

/// Runs the native wait set of one handle for the safe run policy.
struct NativeRun<'a> {
    set: &'a HyphaWaitSet,
    slow_callbacks: u64,
    callback_failure: Option<(AttachmentId, HyphaStatus)>,
}

impl RunDriver for NativeRun<'_> {
    fn dispatch(&mut self, budget: u32, limit: Option<Duration>) -> RunReport {
        let set = self.set;
        // The run claimed `running`: a live borrow here is a caller that runs and
        // configures one `WaitSet` at once.
        let (Ok(mut registrations), Ok(cycles), Ok(mut native), Ok(mut callbacks)) = (
            set.registrations.try_borrow_mut(),
            set.cycles.try_borrow(),
            set.native.try_borrow_mut(),
            set.callback_replay.try_borrow_mut(),
        ) else {
            return RunReport {
                reason: StopReason::SetupFailed,
                error: Some(hypha_core::port::PortError::InvalidState),
                attachment: None,
            };
        };
        IN_DISPATCH.with(|active| active.set(true));
        set.shared.set_run_owner(Some(std::thread::current().id()));
        if let Ok(mut attaches) = set.run_attaches.try_borrow_mut() {
            attaches.held = registrations.len();
        }
        let due = Vec::with_capacity(registrations.len());
        let mut dispatcher = CallbackDispatcher {
            registrations: &mut registrations,
            cycles: &cycles,
            waitset: std::ptr::from_ref(set) as usize,
            set,
            node: set.node(),
            batch_size: set.limits.batch_size(),
            warn_nanos: set.warn_nanos,
            warn_interval_nanos: set.warn_interval_nanos,
            slow_callbacks: 0,
            failure: None,
            health: &set.health,
            due,
            detached: &set.pending_detach,
            callbacks: &mut callbacks,
        };
        let report = match limit {
            Some(limit) => native.run_for(budget, limit, &mut dispatcher),
            None => native.run(budget, &mut dispatcher),
        };
        self.slow_callbacks = dispatcher.slow_callbacks;
        self.callback_failure = dispatcher.failure;
        drop(dispatcher);
        // A timer a callback attached after the last wait joins for the next run, before
        // the deferred detaches, which may name it.
        if let Ok(mut attaches) = set.run_attaches.try_borrow_mut() {
            registrations.append(&mut attaches.pending);
        }
        drop(registrations);
        drop(cycles);
        drop(native);
        // Still inside the run, so a release that tries to run it again is refused as a
        // nested run.
        super::registrations::finish_deferred_detaches(set);
        set.shared.set_run_owner(None);
        IN_DISPATCH.with(|active| active.set(false));
        report
    }
}

/// Run `set` on the calling thread until a stop or a failure, or for at most `limit` of
/// host time. A completed run returns its report, even when the run itself failed.
///
/// # Errors
/// `InvalidState` for a run inside a run, or beside one on another thread.
pub fn waitset_run(
    set: &HyphaWaitSet,
    limit: Option<Duration>,
) -> Result<HyphaWaitSetRunReport, Error> {
    if in_dispatch() {
        return Err(Error::InvalidState);
    }
    let mut driver = NativeRun { set, slow_callbacks: 0, callback_failure: None };
    let report =
        set.shared.running.run(set.limits, limit, &mut driver).map_err(|_| Error::InvalidState)?;
    let slow_callbacks = driver.slow_callbacks;
    let callback_failure = driver.callback_failure;

    // The application's failure request wins over whatever ended the loop.
    let failure_requested = set.shared.take_failure();
    let (reason, status, attachment) = match report.reason {
        _ if failure_requested => {
            (HYPHA_RUN_REASON_RUNTIME_FAILED, HyphaStatus::ApplicationFailed, 0)
        }
        StopReason::Requested => (HYPHA_RUN_REASON_STOPPED, HyphaStatus::Ok, 0),
        StopReason::Deadline => (HYPHA_RUN_REASON_DEADLINE, HyphaStatus::Ok, 0),
        StopReason::SetupFailed => (
            HYPHA_RUN_REASON_SETUP_FAILED,
            report.error.map_or(HyphaStatus::Backend, HyphaStatus::from),
            report.attachment.unwrap_or(0),
        ),
        StopReason::RuntimeFailed => match callback_failure {
            Some((attachment, status)) => (HYPHA_RUN_REASON_RUNTIME_FAILED, status, attachment),
            None => (
                HYPHA_RUN_REASON_RUNTIME_FAILED,
                report.error.map_or(HyphaStatus::Backend, HyphaStatus::from),
                report.attachment.unwrap_or(0),
            ),
        },
    };

    // Only a deadline pauses the run; any other end leaves the replay.
    if reason != HYPHA_RUN_REASON_DEADLINE {
        crate::node_progress::withdraw_progress(set.node(), std::ptr::from_ref(set) as usize);
    }

    Ok(HyphaWaitSetRunReport { reason, status: status as i32, attachment, slow_callbacks })
}

/// Ask `set` to stop; see [`WaitSetShared::request_stop`].
///
/// # Errors
/// The transport's error when the stop cannot be signalled.
pub fn waitset_request_stop(set: &HyphaWaitSet) -> Result<bool, Error> {
    set.shared.request_stop()
}
