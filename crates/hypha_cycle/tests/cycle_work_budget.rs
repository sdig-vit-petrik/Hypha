//! Cycle-wide work limits and per-binding overflow behavior.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "a test asserts on known values and fails loudly"
)]

mod common;
use common::RunIteration;
use hypha_cycle::{
    BindingOptions, CaptureMode, CycleHistory, CycleRuntime, CycleTime, InputVerdict,
    QueueOverflowPolicy,
};
use std::sync::{Arc, Mutex};

fn queued(policy: QueueOverflowPolicy) -> BindingOptions {
    BindingOptions {
        mode: CaptureMode::Queued,
        silence_nanos: 0,
        max_samples_per_iteration: 2,
        overflow_policy: policy,
    }
}

fn history(values: &[u8]) -> Arc<Mutex<CycleHistory>> {
    let mut history = CycleHistory::new(16).expect("valid history");
    for value in values {
        let _ = history.push(&[*value], 1, 1);
    }
    Arc::new(Mutex::new(history))
}

#[test]
fn drop_oldest_keeps_newest_assigned_work_and_counts_only_policy_drops() {
    let mut runtime = CycleRuntime::new();
    let _ = runtime.add_binding(history(&[1, 2, 3, 4]), queued(QueueOverflowPolicy::DropOldest));
    runtime.set_arrival_budget(Some(2)).expect("positive budget");
    runtime
        .run_iteration(
            CycleTime { capture_steady_nanos: 2, ..CycleTime::default() },
            || 2,
            None,
            |captures, _| {
                let payloads: Vec<u8> =
                    captures[0].samples.iter().map(|sample| sample.payload[0]).collect();
                assert_eq!(payloads, [3, 4]);
                assert_eq!(captures[0].work_limit_drops, 2);
                assert_eq!(captures[0].lost_since_capture, 0);
                assert_eq!(captures[0].backlog_samples, 0);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("iteration succeeds");
}

#[test]
fn cycle_budget_rotates_fairly_and_stale_samples_consume_work() {
    let mut runtime = CycleRuntime::new();
    let _ = runtime.add_binding(history(&[1]), queued(QueueOverflowPolicy::Carry));
    let _ = runtime.add_binding(history(&[2]), queued(QueueOverflowPolicy::Carry));
    runtime.set_arrival_budget(Some(1)).expect("positive budget");
    // A binding the budget skips has no new sample and is still usable.
    for expected in [[1, 0], [0, 1]] {
        runtime
            .run_iteration(
                CycleTime { capture_steady_nanos: 2, ..CycleTime::default() },
                || 2,
                None,
                |captures, _| {
                    assert_eq!([captures[0].samples.len(), captures[1].samples.len()], expected);
                    assert!(captures.iter().all(|capture| capture.verdict == InputVerdict::Usable));
                    assert!(captures.iter().all(|capture| capture.work_limit_drops == 0));
                    Ok(Vec::new())
                },
                |_, _| Ok(()),
            )
            .expect("iteration succeeds");
    }

    let stale = history(&[3]);
    let fresh = history(&[4]);
    let mut stale_runtime = CycleRuntime::new();
    let _ = stale_runtime.add_binding(
        Arc::clone(&stale),
        BindingOptions { silence_nanos: 10, ..queued(QueueOverflowPolicy::Carry) },
    );
    let _ = stale_runtime.add_binding(Arc::clone(&fresh), queued(QueueOverflowPolicy::Carry));
    stale_runtime.set_arrival_budget(Some(1)).expect("positive budget");
    let mut first_verdicts = Vec::new();
    stale_runtime
        .run_iteration(
            CycleTime { capture_steady_nanos: 100, ..CycleTime::default() },
            || 1,
            None,
            |captures, _| {
                first_verdicts = captures.iter().map(|capture| capture.verdict).collect();
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("iteration succeeds");
    assert_eq!(first_verdicts, [InputVerdict::Stale, InputVerdict::Usable]);
    stale_runtime
        .run_iteration(
            CycleTime { capture_steady_nanos: 100, ..CycleTime::default() },
            || 2,
            None,
            |captures, _| {
                // Nothing new, and the newest sample is still past the limit.
                assert_eq!(captures[0].verdict, InputVerdict::Stale);
                assert_eq!(captures[1].verdict, InputVerdict::Usable);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("iteration succeeds");
}

#[test]
fn latest_bindings_do_not_consume_the_global_arrival_budget() {
    let mut runtime = CycleRuntime::new();
    let latest_history = history(&[1]);
    let queued_history = history(&[2, 3]);
    let _ = runtime.add_binding(
        latest_history,
        BindingOptions {
            mode: CaptureMode::Latest,
            silence_nanos: 0,
            max_samples_per_iteration: 1,
            overflow_policy: QueueOverflowPolicy::Carry,
        },
    );
    let _ = runtime.add_binding(queued_history, queued(QueueOverflowPolicy::Carry));
    runtime.set_arrival_budget(Some(1)).expect("positive budget");
    runtime
        .run_iteration(
            CycleTime { capture_steady_nanos: 2, ..CycleTime::default() },
            || 2,
            None,
            |captures, _| {
                assert_eq!(captures[0].samples.len(), 1);
                assert_eq!(captures[1].samples.len(), 1);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("iteration succeeds");
}

#[test]
fn zero_assigned_budget_applies_drop_policy_without_stealing_work() {
    let mut runtime = CycleRuntime::new();
    let _ = runtime.add_binding(history(&[1]), queued(QueueOverflowPolicy::Carry));
    let _ = runtime.add_binding(history(&[2]), queued(QueueOverflowPolicy::DropOldest));
    runtime.set_arrival_budget(Some(1)).expect("positive budget");
    runtime
        .run_iteration(
            CycleTime { capture_steady_nanos: 2, ..CycleTime::default() },
            || 2,
            None,
            |captures, _| {
                assert_eq!(captures[0].samples.len(), 1);
                assert_eq!(captures[1].samples.len(), 0);
                assert_eq!(captures[1].work_limit_drops, 1);
                assert_eq!(captures[1].backlog_samples, 0);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("iteration succeeds");
}

#[test]
fn ready_bindings_borrow_idle_budget_subject_to_per_binding_caps() {
    let mut runtime = CycleRuntime::new();
    let _ = runtime.add_binding(history(&[1]), queued(QueueOverflowPolicy::Carry));
    let _ = runtime.add_binding(history(&[2, 3, 4, 5]), queued(QueueOverflowPolicy::Carry));
    runtime.set_arrival_budget(Some(4)).expect("positive budget");
    runtime
        .run_iteration(
            CycleTime { capture_steady_nanos: 2, ..CycleTime::default() },
            || 2,
            None,
            |captures, _| {
                assert_eq!(captures[0].samples.len(), 1);
                assert_eq!(captures[1].samples.len(), 2);
                assert_eq!(captures[1].backlog_samples, 2);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("iteration succeeds");
}

#[test]
fn two_bindings_of_one_history_keep_independent_cursors() {
    let shared = history(&[1, 2]);
    let mut runtime = CycleRuntime::new();
    let _ = runtime.add_binding(Arc::clone(&shared), queued(QueueOverflowPolicy::DropOldest));
    let _ = runtime.add_binding(shared, queued(QueueOverflowPolicy::Carry));
    runtime.set_arrival_budget(Some(2)).expect("positive budget");
    runtime
        .run_iteration(
            CycleTime { capture_steady_nanos: 2, ..CycleTime::default() },
            || 2,
            None,
            |captures, _| {
                let left: Vec<u8> =
                    captures[0].samples.iter().map(|sample| sample.payload[0]).collect();
                let right: Vec<u8> =
                    captures[1].samples.iter().map(|sample| sample.payload[0]).collect();
                // The first drops its oldest excess, the second carries it.
                assert_eq!(left, [2]);
                assert_eq!(right, [1]);
                assert_eq!(captures[0].work_limit_drops, 1);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("iteration succeeds");
    runtime
        .run_iteration(
            CycleTime { capture_steady_nanos: 2, ..CycleTime::default() },
            || 3,
            None,
            |captures, _| {
                assert_eq!(captures[0].samples.len(), 0);
                assert_eq!(captures[1].samples[0].payload[0], 2);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("iteration succeeds");
}
