//! Replay steps and supervision: accepted counts, and samples a cut can never admit.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

mod common;
use common::RunIteration;
use hypha_cycle::{
    BindingOptions, CaptureMode, CycleHistory, CycleRuntime, CycleTime, FAULT_STALE,
    QueueOverflowPolicy, SupervisionLimits,
};
use std::sync::{Arc, Mutex};

const MS: u64 = 1_000_000;

fn queued(max_samples_per_iteration: usize) -> BindingOptions {
    BindingOptions {
        mode: CaptureMode::Queued,
        silence_nanos: 0,
        max_samples_per_iteration,
        overflow_policy: QueueOverflowPolicy::Carry,
    }
}

/// One replay step at `cut`, with the inputs cut at that deadline.
fn replay_step(runtime: &mut CycleRuntime, cut: u64) -> Result<(), i32> {
    let time = CycleTime {
        scheduled_nanos: i64::try_from(cut).expect("cut fits"),
        capture_steady_nanos: 0,
        replay_cut_nanos: Some(cut),
        ..CycleTime::default()
    };
    let scheduled = time.scheduled_nanos;
    runtime.run_iteration(time, move || scheduled, Some(0), |_, _| Ok(Vec::new()), |_, _| Ok(()))
}

#[test]
fn replay_steps_over_one_drain_are_not_silent_while_data_exists_in_each_period() {
    // Twenty samples every 10 ms, drained before the first step; each 20 ms step admits two.
    let history = Arc::new(Mutex::new(CycleHistory::new(32).expect("limits")));
    for index in 1..=20_u64 {
        let stamp = index * 10 * MS;
        let payload = [u8::try_from(index).expect("index fits a byte")];
        history.lock().expect("history").push(&payload, stamp, stamp);
    }
    let mut runtime = CycleRuntime::new();
    runtime.add_binding(Arc::clone(&history), queued(8)).expect("binding");
    runtime
        .set_supervision(
            0,
            SupervisionLimits { silence_nanos: 50 * MS, stagnation_nanos: 0 },
            false,
        )
        .expect("supervision");

    for step in 1..=10_u64 {
        let cut = step * 20 * MS;
        replay_step(&mut runtime, cut).expect("replay step");
        let capture = runtime.binding_capture(0).expect("binding");
        // The samples stamped before this cut: 10, 20, ... ms below the cut.
        let admitted = (cut / (10 * MS)).saturating_sub(1);
        assert_eq!(capture.received_total(), admitted, "step {step} counts the admitted samples");
        let faults = capture.supervision().expect("supervised").faults();
        assert_eq!(faults & FAULT_STALE, 0, "step {step} at {cut} ns found no new data");
    }
}

#[test]
fn a_sample_stamped_far_past_the_cut_is_counted_as_ahead() {
    // Wall time stamps are about 1.7e18 ns; a bag cut is near 1e12 ns.
    let wall = 1_700_000_000_000_000_000_u64;
    let mut bag = CycleHistory::new(8).expect("limits");
    bag.push(&[1], 2 * MS, 2 * MS);
    assert_eq!(bag.stamped_beyond(20 * MS, 5 * MS), 0, "a bag sample is not ahead");

    let mut wall_time = CycleHistory::new(8).expect("limits");
    wall_time.push(&[1], wall, wall);
    wall_time.push(&[2], 2 * MS, 2 * MS);
    assert_eq!(wall_time.stamped_beyond(20 * MS, 5_000 * MS), 1, "the wall sample");
    assert_eq!(wall_time.stamped_beyond(20 * MS, 5 * MS), 1, "bound ignored: no");
}
