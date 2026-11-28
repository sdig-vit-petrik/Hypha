//! The trigger bindings of a data-triggered cycle.

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
    FAULT_STALE, InputVerdict, Passed, QueueOverflowPolicy, SupervisionLimits, TriggerPorts,
    TriggerStep,
};
use std::sync::{Arc, Mutex};

const STATUS_INVALID_ARGUMENT: i32 = 1;
const STATUS_RESOURCE_LIMIT: i32 = 13;

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
    let history = Arc::new(Mutex::new(CycleHistory::new(16).expect("valid history")));
    push(&history, stamps);
    history
}

/// Append samples sent at `stamps`, on both clocks.
fn push(history: &Mutex<CycleHistory>, stamps: &[u64]) {
    let mut history = history.lock().expect("history");
    for stamp in stamps {
        let _ = history.push(&stamp.to_le_bytes(), *stamp, *stamp);
    }
}

fn stamps(capture: &BindingCapture) -> Vec<u64> {
    capture.samples.iter().map(|sample| sample.framework_sent_nanos).collect()
}

/// A step on the oldest pending sample of trigger port `port`.
fn fires(port: usize) -> TriggerStep {
    TriggerStep::Sample { port }
}

/// A fallback step whose `due` ports read stale.
fn falls_back(due: &[usize]) -> TriggerStep {
    let due = due.iter().fold(TriggerPorts::default(), |set, &port| set.with(port).expect("fits"));
    TriggerStep::Fallback { due }
}

/// Supervise binding `index` with a silence limit of `silence_nanos`.
fn supervise(runtime: &mut CycleRuntime, index: u32, silence_nanos: u64) {
    let limits = SupervisionLimits { silence_nanos, stagnation_nanos: 0 };
    runtime.set_supervision(index as usize, limits, false).expect("supervision");
}

/// Run `time`; return what each binding saw.
fn run(runtime: &mut CycleRuntime, time: CycleTime) -> Vec<BindingCapture> {
    let mut seen = Vec::new();
    runtime
        .run_iteration(
            time,
            || time.scheduled_nanos,
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

/// Run one replay step at `cut` whose trigger ports hold `trigger`.
fn step(runtime: &mut CycleRuntime, cut: u64, trigger: TriggerStep) -> Vec<BindingCapture> {
    let time = CycleTime {
        scheduled_nanos: i64::try_from(cut).expect("small"),
        replay_cut_nanos: Some(cut),
        trigger_step: Some(trigger),
        ..CycleTime::default()
    };
    run(runtime, time)
}

fn pending(runtime: &CycleRuntime) -> Vec<Vec<i64>> {
    let mut pending = Vec::new();
    runtime.pending_trigger_stamps(&mut pending).expect("pending");
    pending
}

#[test]
fn set_trigger_refuses_a_latest_binding() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[]), latest()).expect("binding");
    assert_eq!(runtime.set_trigger(index as usize), Err(STATUS_INVALID_ARGUMENT));
    assert_eq!(runtime.triggers(), []);
}

#[test]
fn set_trigger_refuses_a_disabled_binding() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_disabled_binding(queued(1)).expect("binding");
    assert_eq!(runtime.set_trigger(index as usize), Err(STATUS_INVALID_ARGUMENT));
    assert_eq!(runtime.set_trigger(7), Err(STATUS_INVALID_ARGUMENT));
}

#[test]
fn set_trigger_keeps_every_trigger_and_a_repeat_is_one() {
    let mut runtime = CycleRuntime::new();
    for _ in 0..3 {
        let _ = runtime.add_binding(history(&[]), queued(1)).expect("binding");
    }
    for index in [2, 0, 2] {
        runtime.set_trigger(index).expect("a queued trigger");
    }
    assert_eq!(runtime.triggers(), [0, 2], "in binding order, whatever the call order");
}

#[test]
fn set_trigger_refuses_a_trigger_on_a_shared_history() {
    let mut runtime = CycleRuntime::new();
    let shared = history(&[10]);
    let first = runtime.add_binding(Arc::clone(&shared), queued(1)).expect("binding");
    let second = runtime.add_binding(shared, queued(1)).expect("binding");
    runtime.set_trigger(first as usize).expect("the first view");
    // Each sample of the shared history would step twice.
    assert_eq!(runtime.set_trigger(second as usize), Err(STATUS_INVALID_ARGUMENT));
    assert_eq!(runtime.triggers(), [first as usize]);
}

#[test]
fn set_trigger_refuses_more_triggers_than_a_cycle_has() {
    let mut runtime = CycleRuntime::new();
    for index in 0..=TriggerPorts::CAPACITY {
        let _ = runtime.add_binding(history(&[]), queued(1)).expect("binding");
        let set = runtime.set_trigger(index);
        let expected =
            if index < TriggerPorts::CAPACITY { Ok(()) } else { Err(STATUS_RESOURCE_LIMIT) };
        assert_eq!(set, expected, "trigger {index}");
    }
}

#[test]
fn pending_trigger_stamps_lists_unread_samples_oldest_first() {
    let mut runtime = CycleRuntime::new();
    assert!(pending(&runtime).is_empty(), "no trigger is fine");
    let index = runtime.add_binding(history(&[10, 20, 30]), queued(1)).expect("binding");
    runtime.set_trigger(index as usize).expect("queued trigger");
    assert_eq!(runtime.triggers(), [index as usize]);
    assert_eq!(pending(&runtime), [vec![10, 20, 30]]);
    let _ = step(&mut runtime, 10, fires(0));
    assert_eq!(pending(&runtime), [vec![20, 30]]);
}

#[test]
fn pending_lists_each_trigger_port_in_its_own_order() {
    let mut runtime = CycleRuntime::new();
    // Two publishers on one topic: the history keeps their arrival order, not their stamps.
    let first = runtime.add_binding(history(&[20, 10]), queued(1)).expect("binding");
    let second = runtime.add_binding(history(&[15]), queued(1)).expect("binding");
    runtime.set_trigger(second as usize).expect("trigger");
    runtime.set_trigger(first as usize).expect("trigger");
    assert_eq!(pending(&runtime), [vec![20, 10], vec![15]]);
}

#[test]
fn a_trigger_step_takes_exactly_the_oldest_trigger_sample() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[10, 20]), queued(8)).expect("binding");
    runtime.set_trigger(index as usize).expect("trigger");
    let seen = step(&mut runtime, 10, fires(0));
    assert_eq!(stamps(&seen[0]), [10]);
}

#[test]
fn a_step_takes_the_firing_sample_and_leaves_the_others_pending() {
    let mut runtime = CycleRuntime::new();
    let a = runtime.add_binding(history(&[10, 20]), queued(8)).expect("binding");
    let b = runtime.add_binding(history(&[10]), queued(8)).expect("binding");
    runtime.set_trigger(a as usize).expect("trigger");
    runtime.set_trigger(b as usize).expect("trigger");
    let seen = step(&mut runtime, 10, fires(0));
    assert_eq!(stamps(&seen[0]), [10]);
    assert_eq!(seen[1].samples, []);
    assert_eq!(pending(&runtime), [vec![20], vec![10]]);
    let seen = step(&mut runtime, 10, fires(1));
    assert_eq!(seen[0].samples, []);
    assert_eq!(stamps(&seen[1]), [10]);
}

#[test]
fn a_tied_sample_is_in_every_trigger_verdict() {
    let mut runtime = CycleRuntime::new();
    let a = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    let b = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    for index in [a, b] {
        supervise(&mut runtime, index, 5);
        runtime.set_trigger(index as usize).expect("trigger");
    }
    // Port 0 fires first; port 1's tied sample, still pending, already counts for it.
    let seen = step(&mut runtime, 10, fires(0));
    assert_eq!(seen[1].verdict, InputVerdict::Usable);
    assert_eq!(seen[1].age_nanos, 0);
}

#[test]
fn a_non_firing_trigger_exactly_at_its_limit_is_usable_with_exact_age() {
    let mut runtime = CycleRuntime::new();
    let a = runtime.add_binding(history(&[110]), queued(1)).expect("binding");
    let b = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    for index in [a, b] {
        supervise(&mut runtime, index, 100);
        runtime.set_trigger(index as usize).expect("trigger");
    }
    let _ = step(&mut runtime, 10, fires(1));
    let seen = step(&mut runtime, 110, fires(0));
    assert_eq!(seen[1].verdict, InputVerdict::Usable, "100 ns old under a 100 ns limit");
    assert_eq!(seen[1].age_nanos, 100);
    assert!(!seen[1].stale());
}

#[test]
fn the_firing_trigger_reads_age_zero() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    supervise(&mut runtime, index, 100);
    runtime.set_trigger(index as usize).expect("trigger");
    let seen = step(&mut runtime, 10, fires(0));
    assert_eq!(seen[0].verdict, InputVerdict::Usable);
    assert_eq!(seen[0].age_nanos, 0);
}

#[test]
fn other_bindings_are_cut_strictly_before_the_trigger_stamp() {
    let mut runtime = CycleRuntime::new();
    let trigger = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    let _ = runtime.add_binding(history(&[9, 10]), latest()).expect("binding");
    runtime.set_trigger(trigger as usize).expect("trigger");
    let seen = step(&mut runtime, 10, fires(0));
    assert_eq!(stamps(&seen[0]), [10]);
    assert_eq!(stamps(&seen[1]), [9]);
}

#[test]
fn other_inputs_stay_cut_before_the_step_with_two_triggers() {
    let mut runtime = CycleRuntime::new();
    let a = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    let _ = runtime.add_binding(history(&[9, 10]), latest()).expect("binding");
    let queue = runtime.add_binding(history(&[8, 9, 10]), queued(8)).expect("binding");
    let b = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    runtime.set_trigger(a as usize).expect("trigger");
    runtime.set_trigger(b as usize).expect("trigger");
    let seen = step(&mut runtime, 10, fires(1));
    assert_eq!(stamps(&seen[1]), [9]);
    assert_eq!(stamps(&seen[queue as usize]), [8, 9]);
    assert_eq!(stamps(&seen[b as usize]), [10]);
}

#[test]
fn a_fallback_step_takes_no_trigger_sample_and_reports_silence() {
    let mut runtime = CycleRuntime::new();
    let trigger = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    runtime.set_trigger(trigger as usize).expect("trigger");
    supervise(&mut runtime, trigger, 100);
    let _ = step(&mut runtime, 10, fires(0));
    let seen = step(&mut runtime, 110, falls_back(&[0]));
    assert!(seen[0].samples.is_empty(), "{:?}", seen[0].samples);
    assert_eq!(seen[0].verdict, InputVerdict::Stale);
    let faults = seen[0].supervision().expect("supervised").faults();
    assert_ne!(faults & FAULT_STALE, 0);
}

#[test]
fn no_trigger_takes_from_the_arrival_budget() {
    let mut runtime = CycleRuntime::new();
    let a = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    let arrivals = history(&[1, 2, 3]);
    let queue = runtime.add_binding(Arc::clone(&arrivals), queued(8)).expect("binding");
    let b = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    runtime.set_trigger(a as usize).expect("trigger");
    runtime.set_trigger(b as usize).expect("trigger");
    runtime.set_arrival_budget(Some(1)).expect("budget");
    let seen = step(&mut runtime, 10, fires(0));
    assert_eq!(seen[a as usize].samples.len(), 1);
    assert_eq!(seen[queue as usize].samples.len(), 1, "the whole budget");
    push(&arrivals, &[4, 5]);
    let seen = step(&mut runtime, 10, fires(1));
    assert_eq!(seen[b as usize].samples.len(), 1);
    assert_eq!(seen[queue as usize].samples.len(), 1, "the whole budget again");
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
        let seen = step(&mut runtime, stamp, fires(0));
        assert_eq!(stamps(&seen[0]), [stamp]);
        assert_eq!(seen[0].counters.work_limit_drops_total, 0);
    }
}

#[test]
fn a_fallback_step_marks_its_trigger_stale() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[]), queued(1)).expect("binding");
    runtime.set_trigger(index as usize).expect("trigger");
    supervise(&mut runtime, index, 100);
    // The schedule ran the fallback at 100; the capture clock says only 100 passed.
    runtime.start_trigger_silence(0);
    let seen = step(&mut runtime, 100, falls_back(&[0]));
    assert!(seen[0].stale(), "the fallback states the staleness the schedule found");
    let supervision = runtime.binding_capture(0).and_then(BindingCapture::supervision);
    assert_eq!(supervision.expect("supervised").faults(), FAULT_STALE);
}

#[test]
fn a_fallback_step_after_a_trigger_sample_reports_the_trigger_stale() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    runtime.set_trigger(index as usize).expect("trigger");
    supervise(&mut runtime, index, 100);
    let _ = step(&mut runtime, 10, fires(0));
    // The newest trigger sits within its limit; the verdict follows the schedule.
    let seen = step(&mut runtime, 109, falls_back(&[0]));
    assert!(seen[0].stale());
    assert_eq!(seen[0].verdict, InputVerdict::Stale);
}

#[test]
fn a_fallback_marks_stale_only_the_due_ports_and_keeps_never_arrived() {
    let mut runtime = CycleRuntime::new();
    let stepped = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    let silent = runtime.add_binding(history(&[]), queued(1)).expect("binding");
    let young = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    for (index, silence) in [(stepped, 100), (silent, 100), (young, 500)] {
        supervise(&mut runtime, index, silence);
        runtime.set_trigger(index as usize).expect("trigger");
    }
    runtime.start_trigger_silence(0);
    let _ = step(&mut runtime, 10, fires(0));
    let _ = step(&mut runtime, 10, fires(2));
    let seen = step(&mut runtime, 110, falls_back(&[0, 1]));
    assert!(seen.iter().all(|capture| capture.samples.is_empty()), "every trigger port is empty");
    assert_eq!(seen[0].verdict, InputVerdict::Stale);
    assert_eq!(seen[1].verdict, InputVerdict::NeverArrived);
    assert!(seen[1].stale());
    assert_eq!(seen[2].verdict, InputVerdict::Usable, "within its own limit");
    assert!(!seen[2].stale());
}

#[test]
fn a_second_trigger_that_never_arrived_counts_its_silence_from_the_first_evaluation() {
    let mut runtime = CycleRuntime::new();
    let short = runtime.add_binding(history(&[]), queued(1)).expect("binding");
    let long = runtime.add_binding(history(&[]), queued(1)).expect("binding");
    for (index, silence) in [(short, 100), (long, 500)] {
        supervise(&mut runtime, index, silence);
        runtime.set_trigger(index as usize).expect("trigger");
    }
    runtime.start_trigger_silence(0);
    let seen = step(&mut runtime, 100, falls_back(&[0]));
    assert_eq!(seen[long as usize].verdict, InputVerdict::NeverArrived);
    assert_eq!(seen[long as usize].age_nanos, 100, "silent since the first evaluation");
    let seen = step(&mut runtime, 600, falls_back(&[0, 1]));
    assert!(seen[long as usize].stale(), "past its own limit at 500");
}

#[test]
fn a_late_pass_consumes_the_oldest_pending_sample_and_counts_it() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[10, 20]), queued(1)).expect("binding");
    runtime.set_trigger(index as usize).expect("trigger");
    runtime.pass_trigger(0, Passed::Late).expect("a pending sample");
    assert_eq!(pending(&runtime), [vec![20]]);
    let capture = runtime.binding_capture(0).expect("capture");
    assert_eq!(capture.counters.age_rejections_total, 1);
    runtime.pass_trigger(0, Passed::Late).expect("the second sample");
    assert_eq!(runtime.pass_trigger(0, Passed::Late), Err(STATUS_INVALID_ARGUMENT), "nothing left");
}

#[test]
fn a_late_pass_consumes_the_oldest_sample_of_the_named_port() {
    let mut runtime = CycleRuntime::new();
    let a = runtime.add_binding(history(&[10, 20]), queued(1)).expect("binding");
    let b = runtime.add_binding(history(&[15]), queued(1)).expect("binding");
    runtime.set_trigger(a as usize).expect("trigger");
    runtime.set_trigger(b as usize).expect("trigger");
    runtime.pass_trigger(1, Passed::Late).expect("a pending sample");
    assert_eq!(pending(&runtime), [vec![10, 20], vec![]]);
    let rejections = |index: u32| {
        runtime.binding_capture(index as usize).expect("capture").counters.age_rejections_total
    };
    assert_eq!((rejections(a), rejections(b)), (0, 1));
    assert_eq!(
        runtime.pass_trigger(2, Passed::Late),
        Err(STATUS_INVALID_ARGUMENT),
        "no third port"
    );
}

#[test]
fn a_superseded_pass_takes_the_oldest_sample_as_a_work_limit_drop_of_the_next_step() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[10, 20, 30]), queued(1)).expect("binding");
    runtime.set_trigger(index as usize).expect("trigger");
    runtime.pass_trigger(0, Passed::Superseded).expect("a pending sample");
    runtime.pass_trigger(0, Passed::Superseded).expect("the second sample");
    assert_eq!(pending(&runtime), [vec![30]]);
    let seen = step(&mut runtime, 30, fires(0));
    assert_eq!(stamps(&seen[0]), [30]);
    assert_eq!(seen[0].work_limit_drops, 2, "reported by the step after them");
    assert_eq!(seen[0].counters.work_limit_drops_total, 2);
    assert_eq!(seen[0].counters.age_rejections_total, 0, "passed, not late");
    assert_eq!(
        runtime.pass_trigger(0, Passed::Superseded),
        Err(STATUS_INVALID_ARGUMENT),
        "nothing left"
    );
    assert_eq!(
        runtime.pass_trigger(1, Passed::Superseded),
        Err(STATUS_INVALID_ARGUMENT),
        "no second port"
    );
}

#[test]
fn an_overrun_counts_on_its_own_trigger_port_only() {
    let mut runtime = CycleRuntime::new();
    let small = Arc::new(Mutex::new(CycleHistory::new(2).expect("valid history")));
    let a = runtime.add_binding(Arc::clone(&small), queued(1)).expect("binding");
    let b = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    runtime.set_trigger(a as usize).expect("trigger");
    runtime.set_trigger(b as usize).expect("trigger");
    // The history of port 0 holds two samples: 10 is gone before the port reads it.
    push(&small, &[10, 20, 30]);
    let seen = step(&mut runtime, 10, fires(1));
    assert_eq!(seen[a as usize].counters.overrun_total, 1);
    assert_eq!(seen[b as usize].counters.overrun_total, 0);
}

#[test]
fn the_firing_sample_is_delivered_even_past_its_limit_at_capture() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    supervise(&mut runtime, index, 100);
    runtime.set_trigger(index as usize).expect("trigger");
    // Host time: the schedule admitted the sample; the capture runs 1 µs after its send.
    let time = CycleTime {
        scheduled_nanos: 10,
        capture_steady_nanos: 1_010,
        trigger_step: Some(fires(0)),
        ..CycleTime::default()
    };
    let seen = run(&mut runtime, time);
    assert_eq!(stamps(&seen[0]), [10], "the schedule's judgement is the one judgement");
    assert_eq!(seen[0].verdict, InputVerdict::Stale);
    assert_eq!(seen[0].counters.age_rejections_total, 0);
}

#[test]
fn a_triggered_cycle_refuses_a_step_that_names_no_trigger_step() {
    let mut runtime = CycleRuntime::new();
    let index = runtime.add_binding(history(&[10]), queued(1)).expect("binding");
    runtime.set_trigger(index as usize).expect("trigger");
    let time =
        CycleTime { scheduled_nanos: 10, replay_cut_nanos: Some(10), ..CycleTime::default() };
    let refused = runtime.run_iteration(time, || 10, None, |_, _| Ok(Vec::new()), |_, _| Ok(()));
    assert_eq!(refused, Err(STATUS_INVALID_ARGUMENT));
}

/// A history whose samples were sent at host monotonic times `sent`, each stamped on the
/// wall clock an epoch later.
fn host_history(sent: &[u64]) -> Arc<Mutex<CycleHistory>> {
    const EPOCH: u64 = 1_700_000_000_000_000_000;
    let mut history = CycleHistory::new(16).expect("valid history");
    for at in sent {
        let _ = history.push(&at.to_le_bytes(), *at, EPOCH + *at);
    }
    Arc::new(Mutex::new(history))
}

fn sent(capture: &BindingCapture) -> Vec<u64> {
    capture.samples.iter().map(|sample| sample.transport_sent_nanos).collect()
}

/// Run one step on host time at monotonic time 100. The step time is monotonic and the
/// framework stamps are wall time, so neither orders two samples; their host send times do.
fn host_step(runtime: &mut CycleRuntime, trigger: TriggerStep) -> Vec<BindingCapture> {
    host_step_at(runtime, 100, trigger)
}

/// [`host_step`] captured at monotonic time `steady`.
fn host_step_at(
    runtime: &mut CycleRuntime,
    steady: i64,
    trigger: TriggerStep,
) -> Vec<BindingCapture> {
    let time = CycleTime {
        scheduled_nanos: steady,
        capture_steady_nanos: steady,
        trigger_step: Some(trigger),
        ..CycleTime::default()
    };
    run(runtime, time)
}

#[test]
fn on_host_time_an_arrival_sent_after_the_firing_sample_waits_for_a_later_step() {
    // Two pairs drained in one turn: each trigger sample follows its partner on the wire.
    let mut runtime = CycleRuntime::new();
    let trigger =
        runtime.add_binding(host_history(&[11, 21]), queued(8)).expect("binding") as usize;
    let partner =
        runtime.add_binding(host_history(&[10, 20]), queued(8)).expect("binding") as usize;
    runtime.set_trigger(trigger).expect("trigger");

    let first = host_step(&mut runtime, TriggerStep::Sample { port: 0 });
    assert_eq!(sent(&first[trigger]), [11]);
    assert_eq!(sent(&first[partner]), [10], "the partner of the second pair is not due yet");

    let second = host_step(&mut runtime, TriggerStep::Sample { port: 0 });
    assert_eq!(sent(&second[trigger]), [21]);
    assert_eq!(sent(&second[partner]), [20]);
}

#[test]
fn on_host_time_the_arrivals_are_cut_at_the_sample_of_the_port_that_fires() {
    // Two trigger ports drained in one turn, and one queued input between their samples.
    let mut runtime = CycleRuntime::new();
    let early = runtime.add_binding(host_history(&[10]), queued(8)).expect("binding") as usize;
    let late = runtime.add_binding(host_history(&[30]), queued(8)).expect("binding") as usize;
    let other =
        runtime.add_binding(host_history(&[5, 20, 40]), queued(8)).expect("binding") as usize;
    runtime.set_trigger(early).expect("trigger");
    runtime.set_trigger(late).expect("trigger");

    let first = host_step(&mut runtime, TriggerStep::Sample { port: 0 });
    assert_eq!(sent(&first[other]), [5]);
    let second = host_step(&mut runtime, TriggerStep::Sample { port: 1 });
    assert_eq!(sent(&second[other]), [20]);
}

#[test]
fn on_host_time_a_fallback_step_takes_every_arrival() {
    let mut runtime = CycleRuntime::new();
    let trigger = runtime.add_binding(host_history(&[]), queued(8)).expect("binding") as usize;
    let other = runtime.add_binding(host_history(&[10, 20]), queued(8)).expect("binding") as usize;
    runtime.set_trigger(trigger).expect("trigger");

    let seen = host_step(&mut runtime, TriggerStep::Fallback { due: TriggerPorts::default() });
    assert_eq!(sent(&seen[other]), [10, 20]);
}

#[test]
fn on_host_time_a_partner_sent_with_the_firing_sample_is_in_its_step() {
    // One step of one upstream cycle stamps all its outputs with one host send time.
    let mut runtime = CycleRuntime::new();
    let trigger =
        runtime.add_binding(host_history(&[100, 200]), queued(8)).expect("binding") as usize;
    let partner =
        runtime.add_binding(host_history(&[100, 200]), queued(8)).expect("binding") as usize;
    runtime.set_trigger(trigger).expect("trigger");

    let first = host_step_at(&mut runtime, 300, TriggerStep::Sample { port: 0 });
    assert_eq!(sent(&first[partner]), [100]);
    let second = host_step_at(&mut runtime, 300, TriggerStep::Sample { port: 0 });
    assert_eq!(sent(&second[partner]), [200]);
}

#[test]
fn on_host_time_arrivals_are_taken_up_to_the_first_one_sent_after_the_firing_sample() {
    // Two publishers on the partner topic: the history is in arrival order, not send order.
    let mut runtime = CycleRuntime::new();
    let trigger =
        runtime.add_binding(host_history(&[20, 40]), queued(8)).expect("binding") as usize;
    let partner =
        runtime.add_binding(host_history(&[10, 30, 15]), queued(8)).expect("binding") as usize;
    runtime.set_trigger(trigger).expect("trigger");

    let first = host_step(&mut runtime, TriggerStep::Sample { port: 0 });
    assert_eq!(sent(&first[partner]), [10], "30 ends the run, so 15 waits behind it");
    assert_eq!(first[partner].backlog_samples, 0);

    let second = host_step(&mut runtime, TriggerStep::Sample { port: 0 });
    assert_eq!(sent(&second[partner]), [30, 15], "nothing is lost and nothing comes twice");
}

#[test]
fn on_host_time_a_drop_under_the_bound_counts_every_sample_it_passes() {
    let mut runtime = CycleRuntime::new();
    let trigger =
        runtime.add_binding(host_history(&[20, 40]), queued(8)).expect("binding") as usize;
    let partner =
        runtime.add_binding(host_history(&[10, 30, 15, 16]), queued(1)).expect("binding") as usize;
    runtime.set_trigger(trigger).expect("trigger");

    let first = host_step(&mut runtime, TriggerStep::Sample { port: 0 });
    assert_eq!(sent(&first[partner]), [10]);
    assert_eq!(first[partner].work_limit_drops, 0);

    let second = host_step(&mut runtime, TriggerStep::Sample { port: 0 });
    assert_eq!(sent(&second[partner]), [16], "one sample a step: the newest of the three due");
    assert_eq!(second[partner].work_limit_drops, 2, "30 and 15 are passed, and both counted");
    assert_eq!(second[partner].counters.overrun_total, 0);
}

#[test]
fn on_host_time_an_input_whose_samples_wait_behind_the_firing_one_stays_usable() {
    // The partner's only sample was sent after the firing one: it has arrived, so the
    // input is not silent, though this step takes nothing from it.
    let mut runtime = CycleRuntime::new();
    let trigger = runtime.add_binding(host_history(&[100]), queued(8)).expect("binding");
    let partner = runtime.add_binding(host_history(&[105]), queued(8)).expect("binding");
    runtime.set_trigger(trigger as usize).expect("trigger");
    supervise(&mut runtime, partner, 50);

    let seen = host_step_at(&mut runtime, 130, TriggerStep::Sample { port: 0 });
    assert_eq!(sent(&seen[partner as usize]), []);
    assert_eq!(seen[partner as usize].verdict, InputVerdict::Usable);
}
