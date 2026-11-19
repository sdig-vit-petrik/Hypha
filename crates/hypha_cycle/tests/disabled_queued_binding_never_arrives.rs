//! A disabled queued input never arrives, and its sample budget is validated.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]
mod common;
use common::RunIteration;
use hypha_cycle::{
    BindingOptions, CaptureMode, CycleRuntime, CycleTime, InputVerdict, QueueOverflowPolicy,
};
fn queued(max_samples_per_iteration: usize) -> BindingOptions {
    BindingOptions {
        mode: CaptureMode::Queued,
        silence_nanos: 0,
        max_samples_per_iteration,
        overflow_policy: QueueOverflowPolicy::Carry,
    }
}
fn schedule(instant: i64) -> CycleTime {
    CycleTime { scheduled_nanos: instant, capture_steady_nanos: instant, ..CycleTime::default() }
}
#[test]
fn a_disabled_queued_binding_never_arrives_and_needs_a_sample_budget() {
    let mut runtime = CycleRuntime::new();
    assert_eq!(runtime.add_disabled_binding(queued(0)), Err(1));
    runtime.add_disabled_binding(queued(2)).expect("binding");
    let result = runtime.run_iteration(
        schedule(10),
        || 10,
        Some(0),
        |captures, _| {
            assert_eq!(captures[0].verdict, InputVerdict::NeverArrived);
            assert_eq!(captures[0].samples.len(), 0);
            Ok(Vec::new())
        },
        |_, _| Ok(()),
    );
    assert_eq!(result, Ok(()));
    let capture = runtime.binding_capture(0).expect("capture");
    assert_eq!(capture.verdict, InputVerdict::NeverArrived);
    assert_eq!(capture.counters.overrun_total, 0);
    assert_eq!(capture.counters.latest_unavailable_iterations, 0);
    assert!(capture.held().is_none());
}
