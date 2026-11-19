//! Ordered output emission validation for one cycle iteration.

mod common;
use common::RunIteration;
use hypha_cycle::{CycleRuntime, CycleTime, OutputError, OutputFailurePolicy};

fn runtime(output_count: usize, per_output_limit: u32, payload_limit: usize) -> CycleRuntime {
    let mut runtime = CycleRuntime::new();
    for _ in 0..output_count {
        let added =
            runtime.add_output(OutputFailurePolicy::FailRun, payload_limit, per_output_limit);
        assert!(added.is_ok(), "output binding failed: {added:?}");
    }
    runtime
}

fn time() -> CycleTime {
    CycleTime { scheduled_nanos: 1, capture_steady_nanos: 1, ..CycleTime::default() }
}

#[test]
fn sparse_repeated_emissions_keep_callback_order() {
    let mut runtime = runtime(3, 2, 8);
    let mut sent = Vec::new();
    let result = runtime.run_iteration(
        time(),
        || 1,
        Some(0),
        |_, _| Ok(vec![(1, vec![11]), (0, vec![21]), (1, vec![12])]),
        |index, payload| {
            sent.push((index, payload.to_vec()));
            Ok(())
        },
    );

    assert_eq!(result, Ok(()));
    assert_eq!(sent, [(1, vec![11]), (0, vec![21]), (1, vec![12])]);
    assert_eq!(runtime.health().output_attempts, 3);
}

#[test]
fn empty_emission_list_is_a_valid_step() {
    let mut runtime = runtime(2, 1, 8);
    let mut send_count = 0;
    let result = runtime.run_iteration(
        time(),
        || 1,
        Some(0),
        |_, _| Ok(Vec::new()),
        |_, _| {
            send_count += 1;
            Ok(())
        },
    );

    assert_eq!(result, Ok(()));
    assert_eq!(send_count, 0);
    assert_eq!(runtime.health().output_attempts, 0);
}

#[test]
fn invalid_later_output_refuses_all_emissions() {
    let mut runtime = runtime(1, 2, 8);
    let mut send_count = 0;
    let result = runtime.run_iteration(
        time(),
        || 1,
        Some(0),
        |_, _| Ok(vec![(0, vec![1]), (1, vec![2])]),
        |_, _| {
            send_count += 1;
            Ok(())
        },
    );

    assert_eq!(result, Err(1));
    assert_eq!(send_count, 0);
    assert_eq!(runtime.health().output_attempts, 0);
}

#[test]
fn oversized_later_payload_refuses_all_emissions() {
    let mut runtime = runtime(2, 2, 1);
    let mut send_count = 0;
    let result = runtime.run_iteration(
        time(),
        || 1,
        Some(0),
        |_, _| Ok(vec![(0, vec![1]), (1, vec![2, 3])]),
        |_, _| {
            send_count += 1;
            Ok(())
        },
    );

    assert_eq!(result, Err(4));
    assert_eq!(send_count, 0);
    assert_eq!(runtime.health().output_attempts, 0);
}

#[test]
fn later_per_output_overflow_refuses_all_emissions() {
    let mut runtime = runtime(2, 1, 8);
    let mut send_count = 0;
    let result = runtime.run_iteration(
        time(),
        || 1,
        Some(0),
        |_, _| Ok(vec![(0, vec![1]), (1, vec![2]), (0, vec![3])]),
        |_, _| {
            send_count += 1;
            Ok(())
        },
    );

    assert_eq!(result, Err(13));
    assert_eq!(send_count, 0);
    assert_eq!(runtime.health().output_attempts, 0);
}

#[test]
fn output_send_failure_keeps_prior_successes_in_call_order() {
    let mut runtime = runtime(2, 2, 8);
    let mut sent = Vec::new();
    let result = runtime.run_iteration(
        time(),
        || 1,
        Some(0),
        |_, _| Ok(vec![(1, vec![11]), (0, vec![21]), (1, vec![12])]),
        |index, payload| {
            sent.push((index, payload.to_vec()));
            if index == 0 { Err(OutputError::Failure(6)) } else { Ok(()) }
        },
    );

    assert_eq!(result, Err(6));
    assert_eq!(sent, [(1, vec![11]), (0, vec![21])]);
    assert_eq!(runtime.health().output_attempts, 2);
}
