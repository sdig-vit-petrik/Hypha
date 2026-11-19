//! Behaviour and boundary tests for Duration.

use hypha_time::Duration;

#[test]
fn construction_units_agree() {
    assert_eq!(Duration::from_nanos(1_500_000_000).nanos(), 1_500_000_000);
    assert_eq!(Duration::from_micros(1_500_000).nanos(), 1_500_000_000);
    assert_eq!(Duration::from_millis(1_500).nanos(), 1_500_000_000);
    assert_eq!(Duration::from_secs(2).nanos(), 2_000_000_000);
    assert_eq!(Duration::from_millis(1_500).nanos(), 1_500_000_000);
    assert!((Duration::from_millis(1_500).seconds() - 1.5).abs() < f64::EPSILON);
}

#[test]
fn every_unit_constructor_agrees_on_its_scale() {
    assert_eq!(Duration::from_millis(100).nanos(), 100_000_000);
    assert_eq!(Duration::from_secs(2).nanos(), 2_000_000_000);
    assert_eq!(Duration::from_micros(500).nanos(), 500_000);
    assert_eq!(Duration::from_nanos(250).nanos(), 250);
    assert_eq!(Duration::from_millis(1_500).nanos(), 1_500_000_000);
}

#[test]
fn negative_unit_constructors_produce_negative_durations() {
    assert_eq!(Duration::from_nanos(-250).nanos(), -250);
    assert_eq!(Duration::from_micros(-500).nanos(), -500_000);
    assert_eq!(Duration::from_millis(-100).nanos(), -100_000_000);
    assert_eq!(Duration::from_secs(-2).nanos(), -2_000_000_000);
    assert_eq!(Duration::from_millis(-1_500).nanos(), -1_500_000_000);
    assert!((Duration::from_millis(-1_500).seconds() - -1.5).abs() < f64::EPSILON);
}

#[test]
fn from_std_round_trip_and_saturation() {
    let span = Duration::from_std(core::time::Duration::from_millis(250));
    assert_eq!(span.nanos(), 250_000_000);

    let max_std = core::time::Duration::MAX;
    let clamped = Duration::from_std(max_std);
    assert_eq!(clamped.nanos(), i64::MAX);

    let just_above_limit = core::time::Duration::from_nanos((i64::MAX as u64) + 1);
    assert_eq!(Duration::from_std(just_above_limit).nanos(), i64::MAX);

    let exact_limit = core::time::Duration::from_nanos(i64::MAX as u64);
    assert_eq!(Duration::from_std(exact_limit).nanos(), i64::MAX);
}

#[test]
fn is_zero_predicate_positive_and_negative_cases() {
    assert!(Duration::ZERO.is_zero());
    assert!(Duration::from_nanos(0).is_zero());
    assert!(!Duration::from_nanos(1).is_zero());
    assert!(!Duration::from_nanos(-1).is_zero());
    assert!(!Duration::from_nanos(i64::MAX).is_zero());
    assert!(!Duration::from_nanos(i64::MIN).is_zero());
}

#[test]
fn is_positive_predicate_positive_and_negative_cases() {
    assert!(Duration::from_nanos(1).is_positive());
    assert!(Duration::from_millis(5).is_positive());
    assert!(Duration::from_nanos(i64::MAX).is_positive());

    assert!(!Duration::ZERO.is_positive());
    assert!(!Duration::from_nanos(-1).is_positive());
    assert!(!Duration::from_millis(-5).is_positive());
    assert!(!Duration::from_nanos(i64::MIN).is_positive());
}

#[test]
fn arithmetic_and_assignment_operators() {
    let hundred = Duration::from_millis(100);
    assert_eq!((hundred + Duration::from_millis(400)).nanos(), Duration::from_millis(500).nanos());
    assert_eq!(
        (Duration::from_secs(1) - Duration::from_millis(250)).nanos(),
        Duration::from_millis(750).nanos()
    );

    let mut span = Duration::from_secs(1);
    span += Duration::from_millis(500);
    assert_eq!(span.nanos(), Duration::from_millis(1_500).nanos());
    span -= Duration::from_secs(1);
    assert_eq!(span.nanos(), Duration::from_millis(500).nanos());
}

#[test]
fn negation_and_minimum_i64_saturation() {
    let hundred = Duration::from_millis(100);
    assert_eq!((-hundred).nanos(), -100_000_000);
    assert_eq!(-(-hundred), hundred);
    assert_eq!(-Duration::ZERO, Duration::ZERO);

    let min_dur = Duration::from_nanos(i64::MIN);
    let saturated = -min_dur;
    assert_eq!(saturated.nanos(), i64::MAX);
}

#[test]
fn arithmetic_saturates_consistently() {
    let huge = Duration::from_nanos(i64::MAX);
    assert_eq!((huge + Duration::from_nanos(1)).nanos(), i64::MAX);
    assert_eq!((-huge - huge).nanos(), i64::MIN);

    let min_dur = Duration::from_nanos(i64::MIN);
    assert_eq!((min_dur - Duration::from_nanos(1)).nanos(), i64::MIN);

    assert_eq!(Duration::from_micros(i64::MAX).nanos(), i64::MAX);
    assert_eq!(Duration::from_micros(i64::MIN).nanos(), i64::MIN);
    assert_eq!(Duration::from_millis(i64::MAX).nanos(), i64::MAX);
    assert_eq!(Duration::from_millis(i64::MIN).nanos(), i64::MIN);
    assert_eq!(Duration::from_secs(i64::MAX).nanos(), i64::MAX);
    assert_eq!(Duration::from_secs(i64::MIN).nanos(), i64::MIN);
}

#[test]
fn duration_ordering() {
    assert!(Duration::from_millis(100) < Duration::from_millis(200));
    assert!(Duration::from_millis(200) > Duration::from_millis(100));
    assert_eq!(Duration::from_millis(100), Duration::from_millis(100));
    assert!(Duration::from_millis(100) <= Duration::from_millis(100));
    assert!(Duration::from_millis(100) >= Duration::from_millis(100));
    assert!(-Duration::from_millis(100) < Duration::ZERO);
    assert!(Duration::ZERO > -Duration::from_millis(100));
}

#[test]
fn duration_display_formatting() {
    assert_eq!(Duration::from_nanos(1_500_000_000).to_string(), "1.500000000");
    assert_eq!(Duration::from_nanos(1).to_string(), "0.000000001");
    assert_eq!(Duration::from_nanos(0).to_string(), "0.000000000");
    assert_eq!(Duration::from_nanos(-1).to_string(), "-0.000000001");
    assert_eq!(Duration::from_nanos(-1_500_000_000).to_string(), "-1.500000000");

    let min_rendered = Duration::from_nanos(i64::MIN).to_string();
    assert_eq!(min_rendered, "-9223372036.854775808");
    let max_rendered = Duration::from_nanos(i64::MAX).to_string();
    assert_eq!(max_rendered, "9223372036.854775807");
}

#[test]
fn duration_usable_at_compile_time() {
    const PERIOD: Duration = Duration::from_millis(100);
    const ZERO: Duration = Duration::ZERO;

    const _: () = assert!(PERIOD.nanos() == 100_000_000);
    const _: () = assert!(ZERO.nanos() == 0);
    const _: () = assert!(ZERO.is_zero());
    const _: () = assert!(PERIOD.is_positive());
}
