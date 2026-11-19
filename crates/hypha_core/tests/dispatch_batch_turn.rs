//! The batch turn of one dispatch pass and the slow-callback warning rules.

use std::time::Duration;

use hypha_core::dispatch::{BatchStep, BatchTurn, is_slow_callback, is_warning_due};

#[test]
fn a_fresh_turn_with_a_positive_batch_size_is_not_full() {
    let turn = BatchTurn::new(4);
    assert!(!turn.is_full());
}

#[test]
fn accepted_frames_count_toward_the_batch_and_the_loop_continues_below_it() {
    let mut turn = BatchTurn::new(2);
    assert!(turn.record(BatchStep::Accepted));
    assert!(!turn.is_full());
}

#[test]
fn the_batch_size_th_accepted_frame_fills_the_turn_and_stops_the_loop() {
    let mut turn = BatchTurn::new(2);
    assert!(turn.record(BatchStep::Accepted));
    assert!(!turn.record(BatchStep::Accepted));
    assert!(turn.is_full());
}

#[test]
fn a_refused_frame_counts_against_the_batch_the_same_as_an_accepted_one() {
    let mut turn = BatchTurn::new(1);
    assert!(!turn.record(BatchStep::Refused));
    assert!(turn.is_full());
}

#[test]
fn no_more_data_stops_the_loop_without_filling_the_turn() {
    let mut turn = BatchTurn::new(4);
    assert!(!turn.record(BatchStep::Exhausted));
    assert!(!turn.is_full());
}

#[test]
fn slow_callback_threshold_is_exclusive_and_zero_disables_it() {
    assert!(!is_slow_callback(Duration::from_millis(50), 0));
    assert!(!is_slow_callback(Duration::from_nanos(10), 10));
    assert!(is_slow_callback(Duration::from_nanos(11), 10));
}

#[test]
fn warning_is_due_the_first_time_and_after_the_interval() {
    let interval = Duration::from_secs(1);
    assert!(is_warning_due(None, interval));
    assert!(!is_warning_due(Some(Duration::from_millis(500)), interval));
    assert!(is_warning_due(Some(Duration::from_secs(1)), interval));
}

#[test]
fn dispatch_limits_refuse_a_zero_limit() {
    use hypha_core::dispatch::{DispatchLimits, LimitError};
    assert_eq!(DispatchLimits::new(0, 64, 128), Err(LimitError::Zero));
    assert_eq!(DispatchLimits::new(8, 0, 128), Err(LimitError::Zero));
    assert_eq!(DispatchLimits::new(8, 64, 0), Err(LimitError::Zero));
}

#[test]
fn dispatch_limits_refuse_a_batch_above_the_budget() {
    use hypha_core::dispatch::{DispatchLimits, LimitError};
    assert_eq!(DispatchLimits::new(65, 64, 128), Err(LimitError::BatchAboveBudget));
    assert!(DispatchLimits::new(64, 64, 128).is_ok());
}

#[test]
fn dispatch_limits_refuse_registrations_above_the_ceiling_and_keep_the_ceiling() {
    use hypha_core::dispatch::{DispatchLimits, LimitError, REGISTRATION_CEILING};
    let above = REGISTRATION_CEILING.wrapping_add(1);
    assert_eq!(DispatchLimits::new(8, 64, above), Err(LimitError::TooManyRegistrations));
    let at_ceiling = DispatchLimits::new(8, 64, REGISTRATION_CEILING);
    assert_eq!(at_ceiling.map(|limits| limits.max_registrations()), Ok(REGISTRATION_CEILING));
}
