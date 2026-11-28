//! Production history/capture behavior shared by cycle bindings.

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
    InputVerdict, OutputError, OutputFailurePolicy, QueueOverflowPolicy,
};
use std::sync::{Arc, Mutex};

fn queued(max_samples_per_iteration: usize) -> BindingOptions {
    BindingOptions {
        mode: CaptureMode::Queued,
        silence_nanos: 0,
        max_samples_per_iteration,
        overflow_policy: QueueOverflowPolicy::Carry,
    }
}

#[test]
fn shared_bindings_acknowledge_overrun_independently() {
    let mut history = CycleHistory::new(2).expect("valid limits");
    let _ = history.push(&[1], 1, 1);
    let _ = history.push(&[2], 2, 2);
    let mut fast = BindingCapture::new(&history, CaptureMode::Queued, 8).expect("binding");
    let mut slow = BindingCapture::new(&history, CaptureMode::Queued, 8).expect("binding");

    fast.capture(&history, queued(8), 10);
    assert_eq!(fast.samples.len(), 2);
    let _ = history.push(&[3], 3, 3);
    let _ = history.push(&[4], 4, 4);

    fast.capture(&history, queued(8), 10);
    slow.capture(&history, queued(8), 10);
    assert_eq!(fast.lost_since_capture, 0);
    assert_eq!(fast.counters.overrun_total, 0);
    assert_eq!(slow.lost_since_capture, 2);
    assert_eq!(slow.counters.overrun_total, 2);
}

#[test]
fn queued_capture_keeps_backlog_and_consumes_only_examined_samples() {
    let mut history = CycleHistory::new(8).expect("valid limits");
    for sequence in 0..5 {
        let _ = history.push(&[u8::try_from(sequence).expect("small")], sequence, sequence);
    }
    let mut binding = BindingCapture::new(&history, CaptureMode::Queued, 8).expect("binding");

    binding.capture(&history, queued(2), 10);
    assert_eq!(binding.samples.len(), 2);
    assert_eq!(binding.backlog_samples, 3);
    assert_eq!(binding.verdict, InputVerdict::Usable);
    assert_eq!(binding.samples[0].payload.to_vec(), &[0]);

    binding.capture(&history, queued(2), 10);
    assert_eq!(binding.samples.len(), 2);
    assert_eq!(binding.samples[0].payload.to_vec(), &[2]);
    assert_eq!(binding.backlog_samples, 1);
}

#[test]
fn binding_age_limits_are_independent_and_latest_does_not_advance_queue() {
    let mut history = CycleHistory::new(4).expect("valid limits");
    let _ = history.push(&[7], 100, 100);
    let mut strict = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("binding");
    let mut permissive = BindingCapture::new(&history, CaptureMode::Queued, 4).expect("binding");
    strict.capture(
        &history,
        BindingOptions {
            mode: CaptureMode::Latest,
            silence_nanos: 10,
            max_samples_per_iteration: 1,
            overflow_policy: QueueOverflowPolicy::Carry,
        },
        120,
    );
    assert_eq!(strict.verdict, InputVerdict::Stale);
    permissive.capture(
        &history,
        BindingOptions {
            mode: CaptureMode::Queued,
            silence_nanos: 30,
            max_samples_per_iteration: 4,
            overflow_policy: QueueOverflowPolicy::Carry,
        },
        120,
    );
    assert_eq!(permissive.verdict, InputVerdict::Usable);
    assert_eq!(permissive.samples.len(), 1);
}

#[test]
fn captured_views_share_the_owned_payload() {
    let mut history = CycleHistory::new(4).expect("valid limits");
    let _ = history.push(&[1, 2, 3], 1, 1);
    let mut first = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("binding");
    let mut second = BindingCapture::new(&history, CaptureMode::Latest, 1).expect("binding");
    let options = BindingOptions {
        mode: CaptureMode::Latest,
        silence_nanos: 0,
        max_samples_per_iteration: 1,
        overflow_policy: QueueOverflowPolicy::Carry,
    };
    first.capture(&history, options, 2);
    second.capture(&history, options, 2);
    assert!(std::sync::Arc::ptr_eq(&first.samples[0], &second.samples[0]));
}

#[test]
fn shared_history_is_unlocked_during_step_and_both_bindings_capture() {
    let history = Arc::new(Mutex::new(CycleHistory::new(4).expect("valid limits")));
    {
        let mut held = history.lock().expect("history lock");
        let _ = held.push(&[1], 1, 1);
        let _ = held.push(&[2], 2, 2);
    }
    let mut runtime = CycleRuntime::new();
    runtime
        .add_binding(
            Arc::clone(&history),
            BindingOptions {
                mode: CaptureMode::Latest,
                silence_nanos: 0,
                max_samples_per_iteration: 2,
                overflow_policy: QueueOverflowPolicy::Carry,
            },
        )
        .expect("latest binding");
    runtime.add_binding(Arc::clone(&history), queued(2)).expect("queued binding");

    let result = runtime.run_iteration(
        CycleTime { capture_steady_nanos: 3, ..CycleTime::default() },
        || 3,
        Some(0),
        |captures, _| {
            assert!(history.try_lock().is_ok(), "history guard must be released before step");
            assert_eq!(captures[0].samples.len(), 1);
            assert_eq!(captures[0].samples[0].payload.to_vec(), &[2]);
            assert_eq!(captures[1].samples.len(), 2);
            assert_eq!(captures[1].samples[0].payload.to_vec(), &[1]);
            assert_eq!(captures[1].samples[1].payload.to_vec(), &[2]);
            Ok(Vec::new())
        },
        |_, _| Ok(()),
    );

    assert_eq!(result, Ok(()));
}

#[test]
fn queued_cursors_wrap_across_the_u64_boundary() {
    let mut history = CycleHistory::with_initial_sequence(4, u64::MAX - 1).expect("valid limits");
    let _ = history.push(&[1], 1, 1);
    let _ = history.push(&[2], 2, 2);
    let _ = history.push(&[3], 3, 3);
    let mut binding = BindingCapture::new(&history, CaptureMode::Queued, 4).expect("binding");

    binding.capture(&history, queued(4), 10);

    assert_eq!(binding.samples.len(), 3);
    assert_eq!(binding.samples[0].sequence, u64::MAX - 1);
    assert_eq!(binding.samples[1].sequence, u64::MAX);
    assert_eq!(binding.samples[2].sequence, 0);
    assert_eq!(binding.backlog_samples, 0);
}

#[test]
fn production_driver_injects_time_and_dispatches_outputs_in_send_order() {
    let history = Arc::new(Mutex::new(CycleHistory::new(4).expect("valid limits")));
    if let Ok(mut held) = history.lock() {
        let _ = held.push(&[42], 100, 100);
    }
    let mut runtime = CycleRuntime::new();
    runtime
        .add_binding(
            Arc::clone(&history),
            BindingOptions {
                mode: CaptureMode::Latest,
                silence_nanos: 0,
                max_samples_per_iteration: 1,
                overflow_policy: QueueOverflowPolicy::Carry,
            },
        )
        .expect("binding");
    let _ = runtime.add_output(OutputFailurePolicy::FailRun, 16, 1).expect("output");
    let _ = runtime.add_output(OutputFailurePolicy::FailRun, 16, 1).expect("output");
    let mut sent = Vec::new();
    let result = runtime.run_iteration(
        CycleTime {
            scheduled_nanos: 90,
            capture_steady_nanos: 120,
            missed_periods: 3,
            ..CycleTime::default()
        },
        || 110,
        Some(0),
        |bindings, time| {
            assert_eq!(time.scheduled_nanos, 90);
            assert_eq!(time.callback_start_nanos, 110);
            assert_eq!(time.capture_steady_nanos, 120);
            assert!(!time.has_elapsed);
            assert_eq!(bindings[0].samples[0].payload.to_vec(), &[42]);
            Ok(vec![(0, vec![1]), (1, vec![2])])
        },
        |index, payload| {
            sent.push((index, payload[0]));
            Ok(())
        },
    );
    assert_eq!(result, Ok(()));
    assert_eq!(sent, [(0, 1), (1, 2)]);
    assert_eq!(runtime.health().iterations, 1);
}

#[test]
fn production_driver_latches_ignored_emission_error_before_any_send() {
    let mut runtime = CycleRuntime::new();
    let _ = runtime.add_output(OutputFailurePolicy::FailRun, 16, 1).expect("output");
    let mut send_count = 0;
    let result = runtime.run_iteration(
        CycleTime {
            scheduled_nanos: 10,
            capture_steady_nanos: 10,
            missed_periods: 0,
            ..CycleTime::default()
        },
        || 10,
        Some(0),
        |_, _| {
            // The FFI emitter latches an error even when its callback ignores it.
            Err(13)
        },
        |_, _| {
            send_count += 1;
            Ok(())
        },
    );
    assert_eq!(result, Err(13));
    assert_eq!(send_count, 0);
    assert_eq!(runtime.health().output_attempts, 0);
    assert_eq!(runtime.health().callback_failures, 1);
}

#[test]
fn production_driver_drops_only_configured_pool_pressure_and_continues() {
    let mut runtime = CycleRuntime::new();
    let _ = runtime.add_output(OutputFailurePolicy::DropPoolExhausted, 16, 1).expect("output");
    let _ = runtime.add_output(OutputFailurePolicy::FailRun, 16, 1).expect("output");
    let mut attempts = Vec::new();
    let result = runtime.run_iteration(
        CycleTime {
            scheduled_nanos: 1,
            capture_steady_nanos: 1,
            missed_periods: 0,
            ..CycleTime::default()
        },
        || 1,
        Some(0),
        |_, _| Ok(vec![(0, vec![1]), (1, vec![2])]),
        |index, _| {
            attempts.push(index);
            if index == 0 { Err(OutputError::PoolExhausted) } else { Ok(()) }
        },
    );
    assert_eq!(result, Ok(()));
    assert_eq!(attempts, [0, 1]);
    assert_eq!(runtime.health().output_pool_drops, 1);
    assert_eq!(runtime.health().output_failures, 1);
    assert_eq!(runtime.health().output_successes, 1);
}

#[test]
fn publication_failure_preserves_prior_sends_without_counting_a_callback_failure() {
    let mut runtime = CycleRuntime::new();
    for _ in 0..3 {
        runtime.add_output(OutputFailurePolicy::FailRun, 8, 1).expect("output");
    }
    let mut sends = Vec::new();
    let result = runtime.run_iteration(
        CycleTime {
            scheduled_nanos: 1,
            capture_steady_nanos: 1,
            missed_periods: 0,
            ..CycleTime::default()
        },
        || 1,
        Some(0),
        |_, _| Ok(vec![(0, vec![1]), (1, vec![2]), (2, vec![3])]),
        |index, _| {
            sends.push(index);
            if index == 1 {
                return Err(OutputError::Failure(6));
            }
            Ok(())
        },
    );
    assert_eq!(result, Err(6));
    assert_eq!(sends, [0, 1]);
    assert_eq!(runtime.health().iterations, 1);
    assert_eq!(runtime.health().output_attempts, 2);
    assert_eq!(runtime.health().output_successes, 1);
    assert_eq!(runtime.health().output_failures, 1);
    assert_eq!(runtime.health().callback_failures, 0);
}

#[test]
fn disabled_latest_bindings_count_unavailable_iterations_like_empty_live_sources() {
    let mut runtime = CycleRuntime::new();
    runtime
        .add_disabled_binding(BindingOptions {
            mode: CaptureMode::Latest,
            silence_nanos: 0,
            max_samples_per_iteration: 1,
            overflow_policy: QueueOverflowPolicy::Carry,
        })
        .expect("binding");
    for instant in [1, 2] {
        assert_eq!(
            runtime.run_iteration(
                CycleTime {
                    scheduled_nanos: instant,
                    capture_steady_nanos: instant,
                    missed_periods: 0,
                    ..CycleTime::default()
                },
                || instant,
                Some(0),
                |captures, _| {
                    assert_eq!(
                        captures.first().map(|capture| capture.verdict),
                        Some(InputVerdict::NeverArrived)
                    );
                    Ok(Vec::new())
                },
                |_, _| Ok(()),
            ),
            Ok(())
        );
    }
    assert_eq!(
        runtime.binding_capture(0).expect("capture").counters.latest_unavailable_iterations,
        2
    );
}

#[test]
fn negative_capture_time_and_any_oversized_output_fail_before_dispatch() {
    let history = Arc::new(Mutex::new(CycleHistory::new(4).expect("valid limits")));
    if let Ok(mut held) = history.lock() {
        let _ = held.push(&[8], 1, 1);
    }
    let mut runtime = CycleRuntime::new();
    runtime
        .add_binding(
            Arc::clone(&history),
            BindingOptions {
                mode: CaptureMode::Queued,
                silence_nanos: 0,
                max_samples_per_iteration: 1,
                overflow_policy: QueueOverflowPolicy::Carry,
            },
        )
        .expect("binding");
    let _ = runtime.add_output(OutputFailurePolicy::FailRun, 4, 1).expect("output");
    let _ = runtime.add_output(OutputFailurePolicy::FailRun, 1, 1).expect("output");
    let mut sends = 0;

    assert_eq!(
        runtime.run_iteration(
            CycleTime {
                scheduled_nanos: 1,
                capture_steady_nanos: -1,
                missed_periods: 0,
                ..CycleTime::default()
            },
            || 1,
            Some(0),
            |_, _| Ok(Vec::new()),
            |_, _| {
                sends += 1;
                Ok(())
            },
        ),
        Err(1)
    );
    assert_eq!(runtime.binding_capture(0).map(|capture| capture.samples.len()), Some(0));

    assert_eq!(
        runtime.run_iteration(
            CycleTime {
                scheduled_nanos: 1,
                capture_steady_nanos: 2,
                missed_periods: 0,
                ..CycleTime::default()
            },
            || 1,
            Some(0),
            |_, _| Ok(vec![(0, vec![1]), (1, vec![2, 3])]),
            |_, _| {
                sends += 1;
                Ok(())
            },
        ),
        Err(4)
    );
    assert_eq!(sends, 0);
    assert_eq!(runtime.health().output_attempts, 0);
}

#[test]
fn capture_resets_backlog_samples_on_each_capture() {
    let mut history = CycleHistory::new(8).expect("valid limits");
    for sequence in 0..5 {
        let _ = history.push(&[u8::try_from(sequence).expect("small")], sequence, sequence);
    }
    let mut binding = BindingCapture::new(&history, CaptureMode::Queued, 8).expect("binding");
    binding.capture(&history, queued(2), 10);
    assert_eq!(binding.backlog_samples, 3);

    // Latest mode resets backlog_samples to 0.
    let latest_opts = BindingOptions {
        mode: CaptureMode::Latest,
        silence_nanos: 0,
        max_samples_per_iteration: 8,
        overflow_policy: QueueOverflowPolicy::Carry,
    };
    binding.capture(&history, latest_opts, 10);
    assert_eq!(binding.backlog_samples, 0);
}

#[test]
fn capture_latest_resets_previous_verdict_when_no_sample_selected() {
    let mut history = CycleHistory::new(8).expect("valid limits");
    let _ = history.push(&[1], 10, 10);
    let mut binding = BindingCapture::new(&history, CaptureMode::Latest, 8).expect("binding");

    let latest_opts = BindingOptions {
        mode: CaptureMode::Latest,
        silence_nanos: 5,
        max_samples_per_iteration: 8,
        overflow_policy: QueueOverflowPolicy::Carry,
    };

    // 1. Capture usable sample (age 0 <= 5)
    binding.capture(&history, latest_opts, 10);
    assert_eq!(binding.verdict, InputVerdict::Usable);
    assert_eq!(binding.counters.usable_to_stale_transitions, 0);

    // 2. Capture against empty history so selected is None
    let empty_history = CycleHistory::new(8).expect("valid limits");
    binding.capture(&empty_history, latest_opts, 10);
    assert_eq!(binding.verdict, InputVerdict::NeverArrived);

    // 3. Push a stale sample (sent at 10, now at 20, age 10 > 5)
    let _ = history.push(&[2], 10, 10);
    binding.capture(&history, latest_opts, 20);
    assert_eq!(binding.verdict, InputVerdict::Stale);

    // Transition from NeverArrived to Stale should NOT count as usable_to_stale
    assert_eq!(binding.counters.usable_to_stale_transitions, 0);
}

#[test]
fn a_host_step_measures_elapsed_time_on_the_monotonic_clock_when_the_wall_clock_steps_back() {
    // The framework clock reads 5_000 and then 4_000: it stepped back by 1_000 between the
    // steps. The monotonic readings are 1_000 and 1_010, so the gap is 10, never negative.
    let mut runtime = CycleRuntime::new();
    let mut seen: Vec<CycleTime> = Vec::new();
    let first =
        CycleTime { scheduled_nanos: 100, capture_steady_nanos: 1_000, ..CycleTime::default() };
    let result = runtime.run_iteration(
        first,
        || 5_000,
        Some(0),
        |_, time| {
            seen.push(time);
            Ok(vec![])
        },
        |_, _: &[u8]| Ok(()),
    );
    assert_eq!(result, Ok(()));
    let second =
        CycleTime { scheduled_nanos: 110, capture_steady_nanos: 1_010, ..CycleTime::default() };
    let result = runtime.run_iteration(
        second,
        || 4_000,
        Some(0),
        |_, time| {
            seen.push(time);
            Ok(vec![])
        },
        |_, _: &[u8]| Ok(()),
    );
    assert_eq!(result, Ok(()));
    assert!(!seen[0].has_elapsed, "the first step has no gap");
    assert!(seen[1].has_elapsed);
    assert_eq!(seen[1].elapsed_nanos, 10);
}

#[test]
fn the_history_keeps_the_newest_evicted_stamp() {
    let mut history = hypha_cycle::CycleHistory::new(2).expect("a history");
    assert_eq!(history.newest_evicted_sent_nanos(), None);
    history.push(&[1], 0, 10);
    history.push(&[2], 0, 20);
    assert_eq!(history.newest_evicted_sent_nanos(), None, "nothing evicted yet");
    history.push(&[3], 0, 30);
    assert_eq!(history.newest_evicted_sent_nanos(), Some(10));
    history.push(&[4], 0, 5);
    assert_eq!(history.newest_evicted_sent_nanos(), Some(20));
}
