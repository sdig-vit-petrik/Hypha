//! Held sample, received and delivered counts, and message stamp validity.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

mod common;
use common::RunIteration;
use hypha_cycle::{
    BindingCapture, BindingOptions, CaptureMode, CycleHistory, CycleRuntime, CycleTime,
    QueueOverflowPolicy,
};

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

fn schedule(instant: i64) -> CycleTime {
    CycleTime { scheduled_nanos: instant, capture_steady_nanos: instant, ..CycleTime::default() }
}

#[test]
fn a_latest_binding_keeps_its_held_sample_when_the_next_capture_is_stale() {
    let mut history = CycleHistory::new(4).expect("valid limits");
    let _ = history.push(&[1], 100, 100);
    let mut binding = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("binding");
    binding.capture(&history, latest(10), 105);
    assert_eq!(binding.held().map(|sample| sample.payload.to_vec()), Some(vec![1]));

    let _ = history.push(&[2], 0, 0);
    binding.capture(&history, latest(10), 200);

    assert_eq!(binding.verdict, hypha_cycle::InputVerdict::Stale);
    let held = binding.held().expect("held sample survives a stale capture");
    assert_eq!(held.sequence, 0);
    assert_eq!(held.payload.to_vec(), &[1]);
}

#[test]
fn a_stale_sample_never_becomes_the_held_sample() {
    let mut history = CycleHistory::new(4).expect("valid limits");
    let _ = history.push(&[9], 0, 0);
    let mut latest_binding =
        BindingCapture::new(&history, CaptureMode::Latest, 1).expect("binding");
    let mut queued_binding =
        BindingCapture::new(&history, CaptureMode::Queued, 4).expect("binding");

    latest_binding.capture(&history, latest(10), 100);
    queued_binding.capture(&history, queued(4, 10), 100);

    assert_eq!(latest_binding.verdict, hypha_cycle::InputVerdict::Stale);
    assert_eq!(queued_binding.verdict, hypha_cycle::InputVerdict::Stale);
    assert!(latest_binding.held().is_none());
    assert!(queued_binding.held().is_none());
}

#[test]
fn the_held_sample_survives_eviction_from_a_history_of_depth_one() {
    let mut history = CycleHistory::new(1).expect("valid limits");
    let _ = history.push(&[1], 100, 100);
    let mut binding = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("binding");
    binding.capture(&history, latest(10), 105);
    assert_eq!(binding.held().map(|sample| sample.sequence), Some(0));

    // A newer stale sample evicts the held sample from the depth-one history.
    let _ = history.push(&[2], 0, 0);
    assert!(history.samples().iter().all(|sample| sample.sequence != 0));
    binding.capture(&history, latest(10), 200);

    let held = binding.held().expect("held sample outlives its eviction");
    assert_eq!(held.payload.to_vec(), &[1]);
}

#[test]
fn a_queued_binding_holds_the_newest_sample_of_a_multi_sample_capture() {
    let mut history = CycleHistory::new(8).expect("valid limits");
    let mut binding = BindingCapture::new(&history, CaptureMode::Queued, 8).expect("binding");
    let _ = history.push(&[1], 1, 1);
    let _ = history.push(&[2], 2, 2);
    let _ = history.push(&[3], 3, 3);

    binding.capture(&history, queued(8, 0), 10);

    assert_eq!(binding.samples.len(), 3);
    let held = binding.held().expect("queued capture holds a sample");
    assert_eq!(held.sequence, 2);
    assert_eq!(held.payload.to_vec(), &[3]);
}

#[test]
fn a_new_binding_has_no_held_sample_and_a_disabled_binding_never_has_one() {
    let history = CycleHistory::new(4).expect("valid limits");
    let fresh = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("binding");
    assert!(fresh.held().is_none());
    assert_eq!(fresh.delivered_total(), 0);
    assert_eq!(fresh.received_total(), 0);

    let mut runtime = CycleRuntime::new();
    runtime.add_disabled_binding(latest(0)).expect("binding");
    let result = runtime.run_iteration(
        schedule(10),
        || 10,
        Some(0),
        |captures, _| {
            assert!(captures[0].held().is_none());
            Ok(Vec::new())
        },
        |_, _| Ok(()),
    );
    assert_eq!(result, Ok(()));
    let capture = runtime.binding_capture(0).expect("capture");
    assert!(capture.held().is_none());
    assert_eq!(capture.delivered_total(), 0);
}

#[test]
fn received_total_is_the_accepted_count_at_the_capture_high_water_mark() {
    let mut history = CycleHistory::new(8).expect("valid limits");
    let _ = history.push(&[1], 1, 1);
    let _ = history.push(&[2], 2, 2);
    let _ = history.push(&[3], 3, 3);
    let mut binding = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("binding");
    binding.capture(&history, latest(0), 10);
    assert_eq!(binding.received_total(), 3);

    let _ = history.push(&[4], 4, 4);
    let _ = history.push(&[5], 5, 5);
    assert_eq!(binding.received_total(), 3, "an append after the capture does not change it");

    binding.capture(&history, latest(0), 10);
    assert_eq!(binding.received_total(), 5);
}

#[test]
fn delivered_total_counts_usable_samples_only_and_matches_the_retain_counter() {
    // History depth two overwrites B and C before the binding reads them.
    // E is stale. The retain counter counts usable arrivals in order: A, D.
    let mut history = CycleHistory::new(2).expect("valid limits");
    let mut binding = BindingCapture::new(&history, CaptureMode::Queued, 8).expect("binding");

    let _ = history.push(&[1], 100, 100);
    binding.capture(&history, queued(8, 10), 105);
    assert_eq!(binding.delivered_total(), 1);

    let _ = history.push(&[2], 100, 100);
    let _ = history.push(&[3], 100, 100);
    let _ = history.push(&[4], 100, 100);
    let _ = history.push(&[5], 0, 0);
    binding.capture(&history, queued(8, 10), 105);

    assert_eq!(binding.lost_since_capture, 2);
    assert_eq!(binding.samples.len(), 1);
    assert_eq!(binding.stale_since_capture, 1);
    assert_eq!(binding.delivered_total(), 2);
    assert_eq!(binding.received_total(), 5);
}

#[test]
fn the_message_stamp_is_copied_into_the_step_time_and_marked_valid() {
    let mut runtime = CycleRuntime::new();
    let mut seen = None;
    let result = runtime.run_iteration(
        schedule(10),
        || 10,
        Some(1_500_000_000),
        |_, time| {
            seen = Some((time.message_stamp_nanos, time.message_stamp_valid));
            Ok(Vec::new())
        },
        |_, _| Ok(()),
    );
    assert_eq!(result, Ok(()));
    assert_eq!(seen, Some((1_500_000_000, true)));
}

#[test]
fn a_stamp_that_ros_time_can_hold_is_valid_at_both_edges() {
    let largest_secs_with_nanos = i64::from(u32::MAX) * 1_000_000_000 + 999_999_999;
    for stamp in [0, largest_secs_with_nanos] {
        let mut runtime = CycleRuntime::new();
        let mut valid = None;
        let result = runtime.run_iteration(
            schedule(10),
            || 10,
            Some(stamp),
            |_, time| {
                valid = Some(time.message_stamp_valid);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(valid, Some(true), "stamp {stamp}");
    }
}

#[test]
fn a_negative_or_oversized_stamp_is_invalid_but_the_step_still_runs() {
    // Builder cycles read no stamp, so an invalid stamp must not stop them.
    let first_unrepresentable_secs = i64::from(u32::MAX) * 1_000_000_000 + 1_000_000_000;
    let mut runtime = CycleRuntime::new();
    for stamp in [-1, first_unrepresentable_secs] {
        let mut valid = None;
        let result = runtime.run_iteration(
            schedule(10),
            || 10,
            Some(stamp),
            |_, time| {
                valid = Some(time.message_stamp_valid);
                Ok(Vec::new())
            },
            |_, _| Ok(()),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(valid, Some(false), "stamp {stamp}");
    }
    assert_eq!(runtime.health().iterations, 2);
    assert_eq!(runtime.health().callback_failures, 0);
}

#[test]
fn an_unreadable_clock_gives_an_invalid_stamp_and_the_step_still_runs() {
    let mut runtime = CycleRuntime::new();
    let mut seen = None;
    let result = runtime.run_iteration(
        schedule(10),
        || 10,
        None,
        |_, time| {
            seen = Some((time.message_stamp_nanos, time.message_stamp_valid));
            Ok(Vec::new())
        },
        |_, _| Ok(()),
    );
    assert_eq!(result, Ok(()));
    assert_eq!(seen, Some((0, false)));
}
