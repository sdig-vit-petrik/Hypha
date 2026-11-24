//! The trigger binding of a data-triggered cycle.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "a test asserts on known values and fails loudly"
)]

mod common;
use common::RunIteration;
use hypha_cycle::{
    BindingCapture, BindingOptions, CaptureMode, CycleHistory, CycleRuntime, CycleTime,
    FAULT_STALE, InputVerdict, QueueOverflowPolicy, SupervisionLimits,
};
use std::sync::{Arc, Mutex};

const STATUS_INVALID_ARGUMENT: i32 = 1;

fn options(mode: CaptureMode, arrivals: usize, policy: QueueOverflowPolicy) -> BindingOptions {
    BindingOptions {
        mode,
        silence_nanos: 0,
        max_samples_per_iteration: arrivals,
        overflow_policy: policy,
    }
}

fn queued(arrivals: usize) -> BindingOptions {
    options(CaptureMode::Queued, arrivals, QueueOverflowPolicy::DropOldest)
}

fn latest() -> BindingOptions {
    options(CaptureMode::Latest, 1, QueueOverflowPolicy::DropOldest)
}

/// A history whose samples carry their framework stamp as payload and stamp.
fn history(stamps: &[u64]) -> Arc<Mutex<CycleHistory>> {
    let mut history = CycleHistory::new(16).expect("valid history");
    for stamp in stamps {
        let _ = history.push(&stamp.to_le_bytes(), *stamp, *stamp);
    }
    Arc::new(Mutex::new(history))
}

fn stamps(capture: &BindingCapture) -> Vec<u64> {
    capture.samples.iter().map(|sample| sample.framework_sent_nanos).collect()
}

/// Run one step at `cut` with `take` trigger samples; return what each binding saw.
fn step(runtime: &mut CycleRuntime, cut: u64, take: usize) -> Vec<BindingCapture> {
    let time = CycleTime {
        scheduled_nanos: i64::try_from(cut).expect("small"),
        replay_cut_nanos: Some(cut),
        trigger_samples: Some(take),
        ..CycleTime::default()
    };
    let mut seen = Vec::new();
    runtime
        .run_iteration(
            time,
            || i64::try_from(cut).expect("small"),
            None,
            |captures, _| {
                seen = captures.to_vec();
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("iteration succeeds");
    seen
}

#[test]
fn set_trigger_refuses_a_latest_binding() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[]), latest()).expect("binding");
    assert_eq!(runtime.set_trigger(index as usize), Err(STATUS_INVALID_ARGUMENT));
    assert_eq!(runtime.trigger(), None);
}

#[test]
fn set_trigger_refuses_a_disabled_binding() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_disabled_binding(queued(1)).expect("binding");
    assert_eq!(runtime.set_trigger(index as usize), Err(STATUS_INVALID_ARGUMENT));
    assert_eq!(runtime.set_trigger(7), Err(STATUS_INVALID_ARGUMENT));
}

#[test]
fn pending_trigger_stamps_lists_unread_samples_oldest_first() {
    let mut runtime = CycleRuntime::new();
    let mut pending = vec![99];
    runtime.pending_trigger_stamps(&mut pending).expect("no trigger is fine");
    assert!(pending.is_empty(), "{pending:?}");
    let index = runtime.add_binding(history(&[10, 20, 30]), queued(1)).expect("binding");
    runtime.set_trigger(index as usize).expect("queued trigger");
    assert_eq!(runtime.trigger(), Some(index as usize));
    runtime.pending_trigger_stamps(&mut pending).expect("pending");
    assert_eq!(pending, [10, 20, 30]);
    let _ = step(&mut runtime, 10, 1);
    runtime.pending_trigger_stamps(&mut pending).expect("pending");
    assert_eq!(pending, [20, 30]);
}

#[test]
fn a_trigger_step_takes_exactly_the_oldest_trigger_sample() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[10, 20]), queued(8)).expect("binding");
    runtime.set_trigger(index as usize).expect("trigger");
    let seen = step(&mut runtime, 10, 1);
    assert_eq!(stamps(&seen[0]), [10]);
}

#[test]
fn other_bindings_are_cut_strictly_before_the_trigger_stamp() {
    let mut runtime = CycleRuntime::new();
    let trigger = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    let _ = runtime.add_binding(history(&[9, 10]), latest()).expect("binding");
    runtime.set_trigger(trigger as usize).expect("trigger");
    let seen = step(&mut runtime, 10, 1);
    assert_eq!(stamps(&seen[0]), [10]);
    assert_eq!(stamps(&seen[1]), [9]);
}

#[test]
fn a_fallback_step_takes_no_trigger_sample_and_reports_silence() {
    let mut runtime = CycleRuntime::new();
    let trigger = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    runtime.set_trigger(trigger as usize).expect("trigger");
    runtime
        .set_supervision(
            trigger as usize,
            SupervisionLimits { silence_nanos: 100, stagnation_nanos: 0 },
            false,
        )
        .expect("supervision");
    let _ = step(&mut runtime, 10, 1);
    let seen = step(&mut runtime, 110, 0);
    assert!(seen[0].samples.is_empty(), "{:?}", seen[0].samples);
    assert_eq!(seen[0].verdict, InputVerdict::Stale);
    let faults = seen[0].supervision().expect("supervised").faults();
    assert_ne!(faults & FAULT_STALE, 0);
}

#[test]
fn the_trigger_sample_is_outside_the_arrival_budget() {
    let mut runtime = CycleRuntime::new();
    let trigger = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    let _ = runtime.add_binding(history(&[1, 2, 3]), queued(8)).expect("binding");
    runtime.set_trigger(trigger as usize).expect("trigger");
    runtime.set_arrival_budget(Some(1)).expect("budget");
    let seen = step(&mut runtime, 10, 1);
    assert_eq!(seen[0].samples.len(), 1);
    assert_eq!(seen[1].samples.len(), 1);
}

#[test]
fn the_trigger_ignores_its_arrivals_limit_and_drops_no_arrival() {
    let mut runtime = CycleRuntime::new();
    let trigger = runtime
        .add_binding(
            history(&[10, 20, 30]),
            options(CaptureMode::Queued, 1, QueueOverflowPolicy::DropOldest),
        )
        .expect("binding");
    runtime.set_trigger(trigger as usize).expect("trigger");
    for stamp in [10, 20, 30] {
        let seen = step(&mut runtime, stamp, 1);
        assert_eq!(stamps(&seen[0]), [stamp]);
        assert_eq!(seen[0].counters.work_limit_drops_total, 0);
    }
}

#[test]
fn a_fallback_step_marks_its_trigger_stale() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[]), queued(1)).expect("binding");
    runtime.set_trigger(index as usize).expect("trigger");
    let limits = SupervisionLimits { silence_nanos: 100, stagnation_nanos: 0 };
    runtime.set_supervision(index as usize, limits, false).expect("supervision");
    // The schedule ran the fallback at 100; the capture clock says only 100 passed.
    runtime.start_silence(index as usize, 0);
    let seen = step(&mut runtime, 100, 0);
    assert!(seen[0].stale(), "the fallback states the staleness the schedule found");
    let supervision = runtime.binding_capture(0).and_then(BindingCapture::supervision);
    assert_eq!(supervision.expect("supervised").faults(), FAULT_STALE);
}

#[test]
fn a_fallback_step_after_a_trigger_sample_reports_the_trigger_stale() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    runtime.set_trigger(index as usize).expect("trigger");
    let limits = SupervisionLimits { silence_nanos: 100, stagnation_nanos: 0 };
    runtime.set_supervision(index as usize, limits, false).expect("supervision");
    let _ = step(&mut runtime, 10, 1);
    // The newest trigger sits exactly at its limit; the verdict follows the schedule.
    let seen = step(&mut runtime, 109, 0);
    assert!(seen[0].stale());
    assert_eq!(seen[0].verdict, InputVerdict::Stale);
}

#[test]
fn refuse_trigger_consumes_the_oldest_pending_sample_and_counts_it() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[10, 20]), queued(1)).expect("binding");
    runtime.set_trigger(index as usize).expect("trigger");
    runtime.refuse_trigger().expect("a pending sample");
    let mut pending = Vec::new();
    runtime.pending_trigger_stamps(&mut pending).expect("stamps");
    assert_eq!(pending, vec![20]);
    let capture = runtime.binding_capture(0).expect("capture");
    assert_eq!(capture.counters.age_rejections_total, 1);
    runtime.refuse_trigger().expect("the second sample");
    assert_eq!(runtime.refuse_trigger(), Err(STATUS_INVALID_ARGUMENT), "nothing left");
}
