//! A step clears an input (#213): it forgets the held value and the samples that its
//! capture admitted and left pending; a sample after the clear is judged as new.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

mod common;
use common::RunIteration;
use hypha_cycle::{
    BindingOptions, CaptureMode, CycleHistory, CycleRuntime, CycleTime, FAULT_STALE, InputVerdict,
    PinnedBytes, PinnedPayload, QueueOverflowPolicy, SampleOrigin, SupervisionLimits,
};
use std::sync::{Arc, Mutex};

const MS: u64 = 1_000_000;
const SILENCE: u64 = 50 * MS;

fn options(mode: CaptureMode, per_step: usize) -> BindingOptions {
    BindingOptions {
        mode,
        silence_nanos: 0,
        max_samples_per_iteration: per_step,
        overflow_policy: QueueOverflowPolicy::Carry,
    }
}

/// A runtime with one binding of `mode` over `history`, supervised with [`SILENCE`].
fn runtime_over(
    history: &Arc<Mutex<CycleHistory>>,
    mode: CaptureMode,
    per_step: usize,
) -> CycleRuntime {
    let mut runtime = CycleRuntime::new();
    runtime.add_binding(Arc::clone(history), options(mode, per_step)).expect("binding");
    let limits = SupervisionLimits { silence_nanos: SILENCE, stagnation_nanos: 0 };
    runtime.set_supervision(0, limits, false).expect("supervision");
    runtime
}

/// What one step saw of binding 0.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    payloads: Vec<u8>,
    held: Option<u8>,
    verdict: InputVerdict,
    stale: bool,
}

fn step(runtime: &mut CycleRuntime, time: CycleTime) -> Seen {
    let scheduled = time.scheduled_nanos;
    let mut seen = None;
    runtime
        .run_iteration(
            time,
            move || scheduled,
            Some(0),
            |captures, _| {
                let capture = &captures[0];
                seen = Some(Seen {
                    payloads: capture
                        .samples
                        .iter()
                        .map(|sample| sample.payload.with_bytes(|bytes| bytes[0]))
                        .collect(),
                    held: capture.held().map(|sample| sample.payload.with_bytes(|bytes| bytes[0])),
                    verdict: capture.verdict,
                    stale: capture.stale(),
                });
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        )
        .expect("step");
    seen.expect("the step ran")
}

fn host_step(runtime: &mut CycleRuntime, at: u64) -> Seen {
    let at = i64::try_from(at).expect("time fits");
    step(
        runtime,
        CycleTime { scheduled_nanos: at, capture_steady_nanos: at, ..CycleTime::default() },
    )
}

fn replay_step(runtime: &mut CycleRuntime, cut: u64) -> Seen {
    let scheduled = i64::try_from(cut).expect("cut fits");
    step(
        runtime,
        CycleTime {
            scheduled_nanos: scheduled,
            replay_cut_nanos: Some(cut),
            ..CycleTime::default()
        },
    )
}

fn push(history: &Arc<Mutex<CycleHistory>>, value: u8, sent: u64) {
    let _ = history.lock().expect("history").push(&[value], sent, sent);
}

#[test]
fn a_cleared_queued_input_drops_its_backlog_and_takes_only_later_samples() {
    let history = Arc::new(Mutex::new(CycleHistory::new(16).expect("limits")));
    for value in 1..=5 {
        push(&history, value, u64::from(value) * MS);
    }
    let mut runtime = runtime_over(&history, CaptureMode::Queued, 2);

    let first = host_step(&mut runtime, 10 * MS);
    assert_eq!(first.payloads, [1, 2]);
    assert_eq!(runtime.clear_binding(0), Ok(3), "3, 4 and 5 were pending");
    let counters = runtime.binding_capture(0).expect("binding").counters;
    assert_eq!(counters.cleared_total, 3, "the dropped samples are counted");
    assert_eq!(counters.work_limit_drops_total, 0, "and in no other counter");

    let quiet = host_step(&mut runtime, 20 * MS);
    let waiting =
        Seen { payloads: vec![], held: None, verdict: InputVerdict::NeverArrived, stale: false };
    assert_eq!(quiet, waiting, "nothing since the clear, and the source is not silent yet");

    push(&history, 6, 25 * MS);
    let next = host_step(&mut runtime, 30 * MS);
    assert_eq!((next.payloads, next.verdict), (vec![6], InputVerdict::Usable));
    let capture = runtime.binding_capture(0).expect("binding");
    assert_eq!(capture.received_total(), 6, "received counts every sample, cleared or not");
    assert_eq!(capture.delivered_total(), 3, "delivered counts what a step took");
}

#[test]
fn a_cleared_latest_input_has_no_value_until_a_later_sample() {
    let history = Arc::new(Mutex::new(CycleHistory::new(16).expect("limits")));
    push(&history, 1, MS);
    let mut runtime = runtime_over(&history, CaptureMode::Latest, 1);

    assert_eq!(host_step(&mut runtime, 10 * MS).held, Some(1));
    assert_eq!(runtime.clear_binding(0), Ok(0), "a latest input has no backlog");

    let after = host_step(&mut runtime, 20 * MS);
    assert_eq!(
        after,
        Seen { payloads: vec![], held: None, verdict: InputVerdict::NeverArrived, stale: false }
    );

    push(&history, 2, 25 * MS);
    let next = host_step(&mut runtime, 30 * MS);
    assert_eq!((next.held, next.verdict), (Some(2), InputVerdict::Usable));
}

#[test]
fn the_silence_of_a_cleared_input_counts_from_its_newest_sample_before_the_clear() {
    for mode in [CaptureMode::Latest, CaptureMode::Queued] {
        let history = Arc::new(Mutex::new(CycleHistory::new(16).expect("limits")));
        push(&history, 1, MS);
        let mut runtime = runtime_over(&history, mode, 4);
        host_step(&mut runtime, 10 * MS);
        runtime.clear_binding(0).expect("clear");

        // 1 ms + 50 ms: the newest sample is exactly at its limit, still not silent.
        let at_limit = host_step(&mut runtime, MS + SILENCE);
        assert_eq!(
            (at_limit.verdict, at_limit.stale),
            (InputVerdict::NeverArrived, false),
            "{mode:?}"
        );
        // Past it the input is stale, as it would be without the clear: a clear never
        // restarts the silence of a source that stopped.
        let past = host_step(&mut runtime, MS + SILENCE + 1);
        assert_eq!(
            (past.verdict, past.stale, past.held),
            (InputVerdict::Stale, true, None),
            "{mode:?}"
        );
        let faults =
            runtime.binding_capture(0).and_then(|c| c.supervision()).expect("supervised").faults();
        assert_eq!(faults & FAULT_STALE, FAULT_STALE, "{mode:?}");

        push(&history, 2, 70 * MS);
        let back = host_step(&mut runtime, 71 * MS);
        assert_eq!((back.verdict, back.stale), (InputVerdict::Usable, false), "{mode:?}");
    }
}

#[test]
fn a_clear_before_any_sample_keeps_the_count_from_the_first_evaluation() {
    let history = Arc::new(Mutex::new(CycleHistory::new(16).expect("limits")));
    let mut runtime = runtime_over(&history, CaptureMode::Queued, 4);
    host_step(&mut runtime, 10 * MS);
    assert_eq!(runtime.clear_binding(0), Ok(0));
    let never = host_step(&mut runtime, 10 * MS + SILENCE + 1);
    assert_eq!((never.verdict, never.stale), (InputVerdict::NeverArrived, true));
}

#[test]
fn an_unknown_binding_cannot_be_cleared() {
    let mut runtime = CycleRuntime::new();
    assert!(runtime.clear_binding(0).is_err());
}

/// What a 20 ms replay cycle sees of a 10 ms topic stamped 10..=200 ms, when `deliver`
/// says, for each step's cut, what reached the history before it, and the step at 100 ms
/// clears the input.
fn replay_with_clear(mode: CaptureMode, deliver: impl Fn(u64) -> u64) -> (Vec<Seen>, u64) {
    let history = Arc::new(Mutex::new(CycleHistory::new(64).expect("limits")));
    let mut runtime = runtime_over(&history, mode, 1);
    let mut next = 10 * MS;
    let mut seen = Vec::new();
    let mut cleared = 0;
    for cut in (20..=240).step_by(20).map(|ms| ms * MS) {
        while next <= deliver(cut).min(200 * MS) {
            push(&history, u8::try_from(next / MS).expect("value"), next);
            next += 10 * MS;
        }
        seen.push(replay_step(&mut runtime, cut));
        if cut == 100 * MS {
            cleared = runtime.clear_binding(0).expect("clear");
        }
    }
    (seen, cleared)
}

#[test]
fn a_replay_clear_drops_what_the_step_cut_admitted_in_every_chunking() {
    for mode in [CaptureMode::Queued, CaptureMode::Latest] {
        // Every sample through each cut, one chunk ahead, and all of them at once.
        let through_cut = replay_with_clear(mode, |cut| cut);
        let chunk_ahead = replay_with_clear(mode, |cut| cut + 100 * MS);
        let all_at_once = replay_with_clear(mode, |_| u64::MAX);
        assert_eq!(through_cut, chunk_ahead, "{mode:?}");
        assert_eq!(through_cut, all_at_once, "{mode:?}");
        assert_eq!(through_cut, replay_with_clear(mode, |cut| cut), "{mode:?}: a second run");

        let (seen, cleared) = through_cut;
        // The step at 120 ms: 100 and 110 were sent after the clear's cut of 100 ms.
        let after = &seen[5];
        match mode {
            CaptureMode::Queued => {
                // Per step one sample: the step at 100 took 50; 60..=90 were pending.
                assert_eq!(seen[4].payloads, [50]);
                assert_eq!(cleared, 4);
                assert_eq!(after.payloads, [100]);
            }
            CaptureMode::Latest => {
                assert_eq!(seen[4].held, Some(90));
                assert_eq!(cleared, 0);
                assert_eq!(after.held, Some(110));
            }
        }
        assert_eq!(after.verdict, InputVerdict::Usable, "{mode:?}");
    }
}

#[test]
fn a_trigger_port_cannot_be_cleared() {
    let history = Arc::new(Mutex::new(CycleHistory::new(16).expect("limits")));
    let mut runtime = runtime_over(&history, CaptureMode::Queued, 4);
    runtime.set_trigger(0).expect("trigger");
    assert!(runtime.clear_binding(0).is_err(), "its pending samples are planned steps");
}

struct Slot(Vec<u8>);

impl PinnedBytes for Slot {
    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

#[test]
fn a_cleared_sample_read_in_place_counts_as_taken_so_its_slot_can_go_back() {
    let history = Arc::new(Mutex::new(CycleHistory::new(16).expect("limits")));
    let pins: Vec<Arc<PinnedPayload>> =
        (1..=4_u8).map(|value| PinnedPayload::new(Box::new(Slot(vec![value])))).collect();
    for (value, pin) in (1_u64..).zip(&pins) {
        let sent = value * MS;
        let _ = history.lock().expect("history").push_pinned(
            Arc::clone(pin),
            sent,
            sent,
            SampleOrigin::default(),
        );
    }
    let mut runtime = runtime_over(&history, CaptureMode::Queued, 1);

    let first = host_step(&mut runtime, 10 * MS);
    assert_eq!(first.payloads, [1]);
    assert_eq!(runtime.clear_binding(0), Ok(3), "2, 3 and 4 were pending");
    let taken: Vec<u32> = pins.iter().map(|pin| pin.taken()).collect();
    assert_eq!(taken, [1, 1, 1, 1], "the one queued binding reads none of them again");
}
