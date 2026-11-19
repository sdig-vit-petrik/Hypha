//! Independent loss accounting when queued sequence numbers wrap.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test fixtures assert known values"
)]

use std::sync::{Arc, Mutex};

mod common;
use common::RunIteration;
use hypha_cycle::{
    BindingOptions, CaptureMode, CycleHistory, CycleRuntime, CycleTime, QueueOverflowPolicy,
};

#[test]
fn policy_drops_and_history_eviction_remain_distinct_across_sequence_wrap() {
    let history = Arc::new(Mutex::new(
        CycleHistory::with_initial_sequence(2, u64::MAX - 1).expect("history"),
    ));
    let mut runtime = CycleRuntime::new();
    runtime
        .add_binding(
            Arc::clone(&history),
            BindingOptions {
                mode: CaptureMode::Queued,
                silence_nanos: 0,
                max_samples_per_iteration: 1,
                overflow_policy: QueueOverflowPolicy::DropOldest,
            },
        )
        .expect("binding");
    {
        let mut history = history.lock().expect("history lock");
        history.push(&[1], 1, 1);
        history.push(&[2], 1, 1);
        history.push(&[3], 1, 1);
    }
    runtime.set_arrival_budget(Some(1)).expect("budget");
    runtime
        .run_iteration(
            CycleTime { capture_steady_nanos: 2, ..CycleTime::default() },
            || 2,
            None,
            |captures, _| {
                let capture = &captures[0];
                assert_eq!(capture.samples.len(), 1);
                assert_eq!(capture.samples[0].sequence, 0);
                assert_eq!(capture.samples[0].payload.as_ref(), &[3]);
                assert_eq!(capture.lost_since_capture, 1);
                assert_eq!(capture.counters.overrun_total, 1);
                assert_eq!(capture.work_limit_drops, 1);
                assert_eq!(capture.counters.work_limit_drops_total, 1);
                assert_eq!(capture.backlog_samples, 0);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("first iteration");
    history.lock().expect("history lock").push(&[4], 2, 2);
    runtime
        .run_iteration(
            CycleTime { capture_steady_nanos: 3, ..CycleTime::default() },
            || 3,
            None,
            |captures, _| {
                let capture = &captures[0];
                assert_eq!(capture.samples.len(), 1);
                assert_eq!(capture.samples[0].payload.as_ref(), &[4]);
                assert_eq!(capture.lost_since_capture, 0);
                assert_eq!(capture.counters.overrun_total, 1);
                assert_eq!(capture.work_limit_drops, 0);
                assert_eq!(capture.counters.work_limit_drops_total, 1);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("second iteration");
}
