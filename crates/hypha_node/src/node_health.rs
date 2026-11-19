//! Execution counters of one node, counted by every `WaitSet` on it.

use std::sync::atomic::{AtomicU64, Ordering};

use hypha_runtime::ExecutionCounters;

use crate::waitset::{HYPHA_ACTION_FAIL, HyphaCallbackResult, status_from_code};

/// The counters behind the `node.*` and `transport.*` metrics.
///
/// The node owns them; each `WaitSet` holds a shared reference from its creation.
#[derive(Debug, Default)]
pub(crate) struct ExecutionHealth {
    counters: ExecutionCounters,
    /// Sample wakes the audit recovered: a sample was taken on a turn no notification started.
    lost_wakes: AtomicU64,
}

impl ExecutionHealth {
    /// Count one sample that arrived without its wake notification.
    pub(crate) fn record_lost_wake(&self) {
        self.lost_wakes.fetch_add(1, Ordering::Relaxed);
    }

    /// Count one callback answer and return it unchanged.
    ///
    /// A nonzero status is a failure whatever the action. The fail action without a
    /// status is recorded as `Backend`.
    pub(crate) fn recorded(&self, result: HyphaCallbackResult) -> HyphaCallbackResult {
        let failed = result.status != 0 || result.action == HYPHA_ACTION_FAIL;
        let failure_status = status_from_code(result.status) as i32;
        self.counters.record_callback(result.status, failed, failure_status);
        result
    }

    pub(crate) fn add_slow_callbacks(&self, count: u64) {
        self.counters.add_slow_callbacks(count);
    }

    /// Record the end of one dispatch sweep at the current framework time.
    pub(crate) fn record_sweep_end(&self) {
        let now = u64::try_from(hypha_time::now().nanos()).unwrap_or(0);
        self.counters.record_sweep_end(now);
    }

    /// The callbacks invoked, the callbacks that failed and the slow callbacks so far.
    pub(crate) fn callback_counts(&self) -> (u64, u64, u64) {
        let snapshot = self.counters.snapshot();
        (snapshot.callbacks_invoked, snapshot.callback_failures, snapshot.slow_callbacks)
    }

    /// Emit the `node.*` metrics, in the order the metrics ABI documents.
    pub(crate) fn emit_metrics(&self, emit: &mut dyn FnMut(&str, u64)) {
        let snapshot = self.counters.snapshot();
        emit("node.callbacks_invoked", snapshot.callbacks_invoked);
        emit("node.callback_failures", snapshot.callback_failures);
        emit("node.slow_callbacks", snapshot.slow_callbacks);
        emit("node.sweeps", snapshot.sweeps);
        emit("node.last_sweep_end_nanos", snapshot.last_sweep_end_nanos);
        emit(
            "node.last_callback_status",
            u64::try_from(snapshot.last_callback_status).unwrap_or(0),
        );
        emit("node.is_sim_time", u64::from(hypha_time::is_sim_time()));
        emit("transport.lost_wakes", self.lost_wakes.load(Ordering::Relaxed));
    }
}
