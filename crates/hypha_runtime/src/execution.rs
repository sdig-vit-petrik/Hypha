//! Atomic counters shared by a node's executor attachments.

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

/// Thread-safe counters maintained by every wait set attached to a node.
#[derive(Debug, Default)]
pub struct ExecutionCounters {
    callbacks_invoked: AtomicU64,
    callback_failures: AtomicU64,
    slow_callbacks: AtomicU64,
    last_sweep_end_nanos: AtomicU64,
    sweeps: AtomicU64,
    last_callback_status: AtomicI32,
}

/// A point-in-time copy of the executor counters.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionSnapshot {
    /// Number of callbacks the executor invoked.
    pub callbacks_invoked: u64,
    /// Number of callbacks that reported a failure.
    pub callback_failures: u64,
    /// Number of callbacks that exceeded their wait-set warning threshold.
    pub slow_callbacks: u64,
    /// Framework time at which the most recent dispatch sweep ended.
    pub last_sweep_end_nanos: u64,
    /// Number of dispatch sweeps that ended: one per `WaitSet` turn that reached its tick.
    pub sweeps: u64,
    /// Status code of the most recent failed callback, or zero if none failed.
    pub last_callback_status: i32,
}

impl ExecutionCounters {
    /// Record one callback result and return the status to report to its caller.
    pub fn record_callback(&self, status: i32, failed: bool, failure_status: i32) -> i32 {
        self.callbacks_invoked.fetch_add(1, Ordering::Relaxed);
        if failed {
            self.callback_failures.fetch_add(1, Ordering::Relaxed);
            self.last_callback_status.store(failure_status, Ordering::Relaxed);
        }
        status
    }

    /// Add callbacks that exceeded the wait set's configured warning threshold.
    pub fn add_slow_callbacks(&self, count: u64) {
        self.slow_callbacks.fetch_add(count, Ordering::Relaxed);
    }

    /// Record the framework-time end of a dispatch sweep.
    pub fn record_sweep_end(&self, framework_nanos: u64) {
        self.last_sweep_end_nanos.store(framework_nanos, Ordering::Relaxed);
        self.sweeps.fetch_add(1, Ordering::Relaxed);
    }

    /// Read all counters using relaxed ordering; the values are independent diagnostics.
    #[must_use]
    pub fn snapshot(&self) -> ExecutionSnapshot {
        ExecutionSnapshot {
            callbacks_invoked: self.callbacks_invoked.load(Ordering::Relaxed),
            callback_failures: self.callback_failures.load(Ordering::Relaxed),
            slow_callbacks: self.slow_callbacks.load(Ordering::Relaxed),
            last_sweep_end_nanos: self.last_sweep_end_nanos.load(Ordering::Relaxed),
            sweeps: self.sweeps.load(Ordering::Relaxed),
            last_callback_status: self.last_callback_status.load(Ordering::Relaxed),
        }
    }
}
