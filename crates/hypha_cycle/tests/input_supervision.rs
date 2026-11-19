//! Staleness, stagnation, ignore and clock rules of input supervision.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

mod common;
use common::RunIteration;
use hypha_cycle::{
    BindingOptions, CaptureMode, CycleHistory, CycleRuntime, CycleTime, FAULT_STAGNANT,
    FAULT_STALE, InputVerdict, JudgementEvent, QueueOverflowPolicy, Supervision, SupervisionLimits,
};
use std::sync::{Arc, Mutex};

const STATUS_INVALID_ARGUMENT: i32 = 1;

fn latest(silence_nanos: u64) -> BindingOptions {
    BindingOptions {
        mode: CaptureMode::Latest,
        silence_nanos,
        max_samples_per_iteration: 1,
        overflow_policy: QueueOverflowPolicy::Carry,
    }
}

fn queued(max_samples_per_iteration: usize, silence_nanos: u64) -> BindingOptions {
    BindingOptions {
        mode: CaptureMode::Queued,
        silence_nanos,
        max_samples_per_iteration,
        overflow_policy: QueueOverflowPolicy::Carry,
    }
}

fn silence(silence_nanos: u64) -> SupervisionLimits {
    SupervisionLimits { silence_nanos, stagnation_nanos: 0 }
}

fn stagnation(stagnation_nanos: u64) -> SupervisionLimits {
    SupervisionLimits { silence_nanos: 0, stagnation_nanos }
}

fn runtime_with(history: &Arc<Mutex<CycleHistory>>, options: BindingOptions) -> CycleRuntime {
    let mut runtime = CycleRuntime::new();
    runtime.add_binding(Arc::clone(history), options).expect("binding");
    runtime
}

fn new_history() -> Arc<Mutex<CycleHistory>> {
    Arc::new(Mutex::new(CycleHistory::new(8).expect("valid limits")))
}

fn push(history: &Arc<Mutex<CycleHistory>>, payload: &[u8], sent_nanos: u64) {
    history.lock().expect("history").push(payload, sent_nanos, sent_nanos);
}

/// One iteration at framework time `scheduled` and host steady time `steady`. Staleness
/// follows `steady`; stagnation follows `scheduled`.
fn tick(runtime: &mut CycleRuntime, scheduled: i64, steady: i64) -> Result<(), i32> {
    let time = CycleTime {
        scheduled_nanos: scheduled,
        capture_steady_nanos: steady,
        ..CycleTime::default()
    };
    runtime.run_iteration(time, move || scheduled, Some(0), |_, _| Ok(Vec::new()), |_, _| Ok(()))
}

fn faults(runtime: &CycleRuntime, index: usize) -> u32 {
    runtime
        .binding_capture(index)
        .and_then(|capture| capture.supervision())
        .expect("supervised binding")
        .faults()
}

#[test]
fn a_port_is_not_stale_before_the_limit_since_its_start() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, silence(1_000), false).expect("supervision");

    assert_eq!(tick(&mut runtime, 5_000, 5_000), Ok(()));
    assert_eq!(tick(&mut runtime, 6_000, 6_000), Ok(()));
    assert_eq!(faults(&runtime, 0), 0, "exactly the limit after the start is not stale");

    assert_eq!(tick(&mut runtime, 6_001, 6_001), Ok(()));
    assert_eq!(faults(&runtime, 0), FAULT_STALE, "past the limit the port is stale");
}

#[test]
fn staleness_follows_the_capture_clock_not_framework_time() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, silence(100), false).expect("supervision");

    // Framework time runs far ahead while host time moves 50 ns: not stale.
    tick(&mut runtime, 0, 0).expect("iteration");
    tick(&mut runtime, 10_000_000, 50).expect("iteration");
    assert_eq!(faults(&runtime, 0), 0);

    // Host time passes the limit while framework time stands still: stale.
    tick(&mut runtime, 0, 101).expect("iteration");
    assert_eq!(faults(&runtime, 0), FAULT_STALE);
}

#[test]
fn an_alive_value_before_the_first_iteration_is_not_stamped() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, stagnation(100), false).expect("supervision");
    runtime.apply_alive(0, 1);

    push(&history, &[1], 1_000_000);
    tick(&mut runtime, 1_000_000, 1_000_000).expect("iteration");
    assert_eq!(faults(&runtime, 0), 0, "no alive value was stamped before the first capture");

    runtime.apply_alive(0, 1);
    tick(&mut runtime, 1_000_099, 1_000_099).expect("iteration");
    assert_eq!(faults(&runtime, 0), 0, "the stamp starts at the first capture instant");
    tick(&mut runtime, 1_000_100, 1_000_100).expect("iteration");
    assert_eq!(faults(&runtime, 0), FAULT_STAGNANT);
}

#[test]
fn a_clock_that_goes_backward_judges_the_newer_sample_stale_and_not_stagnant() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime
        .set_supervision(0, SupervisionLimits { silence_nanos: 100, stagnation_nanos: 100 }, false)
        .expect("supervision");
    push(&history, &[1], 1_000);
    tick(&mut runtime, 1_000, 1_000).expect("iteration");
    runtime.apply_alive(0, 5);
    tick(&mut runtime, 1_050, 1_050).expect("iteration");
    assert_eq!(faults(&runtime, 0), 0);

    tick(&mut runtime, 900, 900).expect("iteration");

    // A sample stamped after the capture clock is not usable: the verdict and the fault
    // say so together.
    assert_eq!(faults(&runtime, 0), FAULT_STALE);
    assert_eq!(runtime.binding_capture(0).expect("capture").verdict, InputVerdict::Stale);
}

#[test]
fn a_fresh_sample_clears_staleness_at_once() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, silence(100), false).expect("supervision");
    tick(&mut runtime, 0, 0).expect("iteration");
    tick(&mut runtime, 101, 101).expect("iteration");
    assert_eq!(faults(&runtime, 0), FAULT_STALE);

    push(&history, &[1], 150);
    tick(&mut runtime, 150, 150).expect("iteration");

    assert_eq!(faults(&runtime, 0), 0, "a sample sent at 150 is fresh at 150");
}

#[test]
fn a_value_exactly_at_the_limit_is_not_stale() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, silence(100), false).expect("supervision");
    push(&history, &[1], 0);

    tick(&mut runtime, 100, 100).expect("iteration");
    assert_eq!(runtime.binding_capture(0).expect("capture").verdict, InputVerdict::Usable);
    assert_eq!(faults(&runtime, 0), 0, "an age equal to the limit is usable");

    tick(&mut runtime, 101, 101).expect("iteration");
    assert_eq!(runtime.binding_capture(0).expect("capture").verdict, InputVerdict::Stale);
    assert_eq!(faults(&runtime, 0), FAULT_STALE, "one nanosecond past it is stale");
}

#[test]
fn a_sample_that_arrives_late_does_not_clear_staleness() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, silence(100), false).expect("supervision");
    tick(&mut runtime, 0, 0).expect("iteration");

    // Sent at 0 and first seen at 150: delivered, but older than the limit.
    push(&history, &[1], 0);
    tick(&mut runtime, 150, 150).expect("iteration");

    let capture = runtime.binding_capture(0).expect("capture");
    assert_eq!(capture.verdict, InputVerdict::Stale);
    assert_eq!(capture.received_total(), 1);
    assert_eq!(capture.counters.age_rejections_total, 1);
    assert_eq!(faults(&runtime, 0), FAULT_STALE, "an old sample proves nothing about now");
}

#[test]
fn the_silence_limit_is_the_binding_limit() {
    let history = new_history();
    // The binding was registered without a limit; supervision sets it.
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, silence(100), false).expect("supervision");
    push(&history, &[1], 0);
    tick(&mut runtime, 101, 101).expect("iteration");
    assert_eq!(runtime.binding_capture(0).expect("capture").verdict, InputVerdict::Stale);
}

#[test]
fn a_limit_without_supervision_judges_the_value_only() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(100));
    push(&history, &[1], 0);
    tick(&mut runtime, 101, 101).expect("iteration");
    let capture = runtime.binding_capture(0).expect("capture");
    assert_eq!(capture.verdict, InputVerdict::Stale);
    assert!(capture.stale());
    assert!(capture.supervision().is_none());
}

#[test]
fn a_counter_that_does_not_change_makes_the_port_stagnant_after_the_limit() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, stagnation(100), false).expect("supervision");
    push(&history, &[1], 10);
    tick(&mut runtime, 10, 10).expect("iteration");
    runtime.apply_alive(0, u64::MAX);

    tick(&mut runtime, 109, 109).expect("iteration");
    assert_eq!(faults(&runtime, 0), 0, "one nanosecond before the stagnation limit");
    tick(&mut runtime, 110, 110).expect("iteration");
    assert_eq!(faults(&runtime, 0), FAULT_STAGNANT);

    // A counter wrap from u64::MAX to 0 is a change.
    runtime.apply_alive(0, 0);
    assert_eq!(faults(&runtime, 0), 0, "a wrap restarts the stagnation timer");
}

#[test]
fn a_port_with_no_sample_is_stale_and_never_stagnant() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime
        .set_supervision(0, SupervisionLimits { silence_nanos: 100, stagnation_nanos: 100 }, false)
        .expect("supervision");
    tick(&mut runtime, 0, 0).expect("iteration");
    runtime.apply_alive(0, 7);

    tick(&mut runtime, 500, 500).expect("iteration");

    assert_eq!(faults(&runtime, 0), FAULT_STALE);
}

#[test]
fn a_queued_port_is_stale_by_its_newest_sample() {
    let history = new_history();
    let mut runtime = runtime_with(&history, queued(4, 0));
    runtime.set_supervision(0, silence(100), false).expect("supervision");
    tick(&mut runtime, 0, 0).expect("iteration");
    tick(&mut runtime, 101, 101).expect("iteration");
    assert_eq!(faults(&runtime, 0), FAULT_STALE);

    push(&history, &[1], 150);
    tick(&mut runtime, 150, 150).expect("iteration");
    assert_eq!(faults(&runtime, 0), 0);

    // A capture with no new sample is usable while the newest sample is within the limit.
    tick(&mut runtime, 250, 250).expect("iteration");
    let capture = runtime.binding_capture(0).expect("capture");
    assert_eq!(capture.verdict, InputVerdict::Usable);
    assert!(capture.samples.is_empty());
    assert_eq!(faults(&runtime, 0), 0);
    tick(&mut runtime, 251, 251).expect("iteration");
    assert_eq!(faults(&runtime, 0), FAULT_STALE);
}

fn verdict(runtime: &CycleRuntime, index: usize) -> InputVerdict {
    runtime.binding_capture(index).expect("capture").verdict
}

#[test]
fn a_silent_queued_port_is_stale_until_a_fresh_sample() {
    let history = new_history();
    let mut runtime = runtime_with(&history, queued(4, 0));
    runtime.set_supervision(0, silence(100), false).expect("supervision");
    tick(&mut runtime, 0, 0).expect("iteration");
    tick(&mut runtime, 101, 101).expect("iteration");
    assert_eq!(verdict(&runtime, 0), InputVerdict::NeverArrived, "nothing arrived yet");
    assert_eq!(faults(&runtime, 0), FAULT_STALE);

    push(&history, &[1], 150);
    tick(&mut runtime, 150, 150).expect("iteration");
    assert_eq!(verdict(&runtime, 0), InputVerdict::Usable);

    // A publisher slower than the cycle: nothing new, and the newest sample is young.
    tick(&mut runtime, 250, 250).expect("iteration");
    assert_eq!(verdict(&runtime, 0), InputVerdict::Usable);
    assert_eq!(faults(&runtime, 0), 0);

    // The publisher stopped: the verdict states the silence, as the fault does.
    for at in [251, 10_000] {
        tick(&mut runtime, at, at).expect("iteration");
        assert_eq!(verdict(&runtime, 0), InputVerdict::Stale, "silent at {at}");
        assert_eq!(faults(&runtime, 0), FAULT_STALE);
    }

    push(&history, &[2], 10_050);
    tick(&mut runtime, 10_050, 10_050).expect("iteration");
    assert_eq!(verdict(&runtime, 0), InputVerdict::Usable);
    assert_eq!(faults(&runtime, 0), 0);
}

#[test]
fn a_queued_port_without_a_silence_limit_is_never_stale() {
    let history = new_history();
    let mut runtime = runtime_with(&history, queued(4, 0));
    push(&history, &[1], 0);
    tick(&mut runtime, 0, 0).expect("iteration");
    tick(&mut runtime, 1_000_000_000, 1_000_000_000).expect("iteration");
    assert_eq!(verdict(&runtime, 0), InputVerdict::Usable);
}

#[test]
fn an_ignored_port_keeps_its_faults_and_is_not_faulted() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, silence(100), true).expect("supervision");
    tick(&mut runtime, 0, 0).expect("iteration");
    tick(&mut runtime, 101, 101).expect("iteration");

    let supervision: &Supervision = runtime
        .binding_capture(0)
        .and_then(|capture| capture.supervision())
        .expect("supervised binding");
    assert!(supervision.ignored());
    assert_eq!(supervision.faults(), FAULT_STALE, "the real condition stays visible");
    assert!(!supervision.faulted(), "an ignored fault does not count");

    runtime.set_ignored(0, false);
    let supervision = runtime
        .binding_capture(0)
        .and_then(|capture| capture.supervision())
        .expect("supervised binding");
    assert!(supervision.faulted());
}

#[test]
fn a_binding_without_supervision_reports_no_faults() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    tick(&mut runtime, 0, 0).expect("iteration");
    tick(&mut runtime, 1_000, 1_000).expect("iteration");
    runtime.apply_alive(0, 3);
    runtime.set_ignored(0, true);

    let capture = runtime.binding_capture(0).expect("capture");
    assert!(capture.supervision().is_none());
}

#[test]
fn supervision_calls_ignore_an_unknown_index() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));

    assert_eq!(
        runtime.set_supervision(5, silence(100), false),
        Err(STATUS_INVALID_ARGUMENT),
        "an unknown index is refused"
    );
    runtime.apply_alive(5, 1);
    runtime.set_ignored(5, true);

    assert!(runtime.binding_capture(0).expect("capture").supervision().is_none());
}

fn take(runtime: &mut CycleRuntime, index: usize) -> [Option<JudgementEvent>; 2] {
    runtime.take_judgement_events(index)
}

fn one(event: JudgementEvent) -> [Option<JudgementEvent>; 2] {
    [Some(event), None]
}

const NOTHING: [Option<JudgementEvent>; 2] = [None, None];

#[test]
fn each_change_is_reported_once_and_a_judgement_that_stays_is_not() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, silence(100), false).expect("supervision");
    tick(&mut runtime, 0, 0).expect("iteration");
    assert_eq!(take(&mut runtime, 0), NOTHING, "the initial wait logs nothing");

    tick(&mut runtime, 101, 101).expect("iteration");
    let never = JudgementEvent::NeverArrived { age_nanos: 101, limit_nanos: 100 };
    assert_eq!(take(&mut runtime, 0), one(never));
    tick(&mut runtime, 200, 200).expect("iteration");
    assert_eq!(take(&mut runtime, 0), NOTHING, "never arrived is logged once");

    push(&history, &[1], 250);
    tick(&mut runtime, 250, 250).expect("iteration");
    assert_eq!(take(&mut runtime, 0), one(JudgementEvent::Usable));
    tick(&mut runtime, 300, 300).expect("iteration");
    assert_eq!(take(&mut runtime, 0), NOTHING, "a steady input logs nothing");

    tick(&mut runtime, 351, 351).expect("iteration");
    let stale = JudgementEvent::Stale { age_nanos: 101, limit_nanos: 100 };
    assert_eq!(take(&mut runtime, 0), one(stale));
    push(&history, &[2], 400);
    tick(&mut runtime, 400, 400).expect("iteration");
    assert_eq!(take(&mut runtime, 0), one(JudgementEvent::UsableAgain));
}

#[test]
fn an_input_without_a_limit_logs_only_its_first_sample() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    tick(&mut runtime, 0, 0).expect("iteration");
    tick(&mut runtime, 1_000_000_000, 1_000_000_000).expect("iteration");
    assert_eq!(take(&mut runtime, 0), NOTHING, "no limit: never arrived is never logged");
    push(&history, &[1], 1_000_000_001);
    tick(&mut runtime, 1_000_000_002, 1_000_000_002).expect("iteration");
    assert_eq!(take(&mut runtime, 0), one(JudgementEvent::Usable));
    tick(&mut runtime, 9_000_000_000, 9_000_000_000).expect("iteration");
    assert_eq!(take(&mut runtime, 0), NOTHING);
}

#[test]
fn a_stuck_alive_value_is_reported_as_stagnant_with_its_time() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    let limits = SupervisionLimits { silence_nanos: 1_000, stagnation_nanos: 100 };
    runtime.set_supervision(0, limits, false).expect("supervision");
    push(&history, &[1], 0);
    tick(&mut runtime, 0, 0).expect("iteration");
    runtime.apply_alive(0, 5);
    assert_eq!(take(&mut runtime, 0), one(JudgementEvent::Usable));

    push(&history, &[1], 120);
    tick(&mut runtime, 120, 120).expect("iteration");
    runtime.apply_alive(0, 5);
    let stagnant = JudgementEvent::Stagnant { unchanged_nanos: 120, limit_nanos: 100 };
    assert_eq!(take(&mut runtime, 0), one(stagnant));

    push(&history, &[2], 130);
    tick(&mut runtime, 130, 130).expect("iteration");
    runtime.apply_alive(0, 6);
    tick(&mut runtime, 140, 140).expect("iteration");
    assert_eq!(take(&mut runtime, 0), one(JudgementEvent::UsableAgain));
}

#[test]
fn an_ignore_change_is_reported_only_while_faulted() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    runtime.set_supervision(0, silence(100), false).expect("supervision");
    push(&history, &[1], 0);
    tick(&mut runtime, 0, 0).expect("iteration");
    assert_eq!(take(&mut runtime, 0), one(JudgementEvent::Usable));
    runtime.set_ignored(0, true);
    assert_eq!(take(&mut runtime, 0), NOTHING, "an ignore change without a fault");
    runtime.set_ignored(0, false);
    assert_eq!(take(&mut runtime, 0), NOTHING);

    tick(&mut runtime, 101, 101).expect("iteration");
    runtime.set_ignored(0, true);
    let stale = JudgementEvent::Stale { age_nanos: 101, limit_nanos: 100 };
    assert_eq!(take(&mut runtime, 0), [Some(stale), Some(JudgementEvent::FaultIgnored)]);
    assert_eq!(take(&mut runtime, 0), NOTHING, "the same override is not reported twice");
    runtime.set_ignored(0, false);
    assert_eq!(take(&mut runtime, 0), [None, Some(JudgementEvent::FaultCountsAgain)]);
}

#[test]
fn an_unknown_index_reports_nothing() {
    let history = new_history();
    let mut runtime = runtime_with(&history, latest(0));
    assert_eq!(take(&mut runtime, 5), NOTHING);
}
