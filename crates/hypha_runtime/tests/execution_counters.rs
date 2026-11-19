//! Execution counters count callbacks, failures, slow callbacks and sweeps.

use hypha_runtime::{ExecutionCounters, ExecutionSnapshot};

#[test]
fn counters_start_at_zero() {
    assert_eq!(ExecutionCounters::default().snapshot(), ExecutionSnapshot::default());
}

#[test]
fn failed_callback_is_counted_and_keeps_its_status() {
    let counters = ExecutionCounters::default();
    assert_eq!(counters.record_callback(0, false, 0), 0);
    assert_eq!(counters.record_callback(7, true, 12), 7);
    counters.add_slow_callbacks(2);
    counters.record_sweep_end(99);
    assert_eq!(
        counters.snapshot(),
        ExecutionSnapshot {
            callbacks_invoked: 2,
            callback_failures: 1,
            slow_callbacks: 2,
            last_sweep_end_nanos: 99,
            sweeps: 1,
            last_callback_status: 12,
        }
    );
}
