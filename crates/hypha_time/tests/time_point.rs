//! Behaviour and boundary tests for Time.

use hypha_time::{Duration, Time};

#[test]
fn default_is_zero_and_zero_means_unknown() {
    let defaulted = Time::default();
    assert!(defaulted.is_zero());
    assert_eq!(defaulted.nanos(), 0);
    assert_eq!(defaulted, Time::ZERO);
    assert_eq!(defaulted, Time::from_nanos(0));
}

#[test]
fn is_zero_predicate_positive_and_negative_cases() {
    assert!(Time::ZERO.is_zero());
    assert!(Time::from_nanos(0).is_zero());

    assert!(!Time::from_nanos(1).is_zero());
    assert!(!Time::from_nanos(-1).is_zero());
    assert!(!Time::from_nanos(1_000_000_000).is_zero());
    assert!(!Time::from_nanos(-1_000_000_000).is_zero());
    assert!(!Time::from_nanos(i64::MAX).is_zero());
    assert!(!Time::from_nanos(i64::MIN).is_zero());
}

#[test]
fn time_seconds_getter_reads_nanoseconds() {
    let instant = Time::from_nanos(412_250_000_000);
    assert!((instant.seconds() - 412.25).abs() < f64::EPSILON);
}

#[test]
fn from_nanos_and_seconds_conversion() {
    let instant = Time::from_nanos(12_500_000_000);
    assert_eq!(instant.nanos(), 12_500_000_000);
    assert!((instant.seconds() - 12.5).abs() < f64::EPSILON);

    let negative_instant = Time::from_nanos(-12_500_000_000);
    assert_eq!(negative_instant.nanos(), -12_500_000_000);
    assert!((negative_instant.seconds() - -12.5).abs() < f64::EPSILON);
}

#[test]
fn difference_between_time_points_yields_duration() {
    let a = Time::from_nanos(10_000_000_000);
    let b = Time::from_nanos(12_500_000_000);
    assert!(((b - a).seconds() - 2.5).abs() < 1e-12);
    assert!(((a - b).seconds() - -2.5).abs() < 1e-12);
    assert_eq!(a - a, Duration::ZERO);
}

#[test]
fn time_difference_saturation() {
    let max_time = Time::from_nanos(i64::MAX);
    let min_time = Time::from_nanos(i64::MIN);

    assert_eq!((max_time - min_time).nanos(), i64::MAX);
    assert_eq!((min_time - max_time).nanos(), i64::MIN);
}

#[test]
fn duration_offsets_and_assignment() {
    let base = Time::from_nanos(100_000_000_000);
    assert!(((base + Duration::from_millis(500)).seconds() - 100.5).abs() < 1e-12);
    assert!(((base - Duration::from_millis(500)).seconds() - 99.5).abs() < 1e-12);
    assert!(((base + Duration::from_millis(-500)).seconds() - 99.5).abs() < 1e-12);
    assert!(((base - Duration::from_millis(-500)).seconds() - 100.5).abs() < 1e-12);

    let mut moving = base;
    moving += Duration::from_secs(1);
    assert!((moving.seconds() - 101.0).abs() < 1e-12);
    moving += Duration::from_secs(-2);
    assert!((moving.seconds() - 99.0).abs() < 1e-12);
}

#[test]
fn arithmetic_saturates_consistently() {
    let late = Time::from_nanos(i64::MAX);
    assert_eq!((late + Duration::from_nanos(10)).nanos(), i64::MAX);
    assert_eq!((late - Duration::from_nanos(-10)).nanos(), i64::MAX);

    let early = Time::from_nanos(i64::MIN);
    assert_eq!((early - Duration::from_nanos(10)).nanos(), i64::MIN);
    assert_eq!((early + Duration::from_nanos(-10)).nanos(), i64::MIN);

    let mut saturating_mut = late;
    saturating_mut += Duration::from_nanos(100);
    assert_eq!(saturating_mut.nanos(), i64::MAX);
}

#[test]
fn time_ordering() {
    assert!(Time::from_nanos(1_000_000_000) < Time::from_nanos(2_000_000_000));
    assert!(Time::from_nanos(2_000_000_000) > Time::from_nanos(1_000_000_000));
    assert!(Time::from_nanos(2_000_000_000) >= Time::from_nanos(2_000_000_000));
    assert!(Time::from_nanos(2_000_000_000) <= Time::from_nanos(2_000_000_000));
    assert_eq!(Time::from_nanos(2_000_000_000), Time::from_nanos(2_000_000_000));

    assert!(Time::from_nanos(-1_000_000_000) < Time::ZERO);
    assert!(Time::ZERO > Time::from_nanos(-1_000_000_000));
    assert!(Time::from_nanos(-2_000_000_000) < Time::from_nanos(-1_000_000_000));
}

#[test]
fn time_display_formatting() {
    assert_eq!(Time::ZERO.to_string(), "0.000000000");
    assert_eq!(Time::from_nanos(12_250_000_000).to_string(), "12.250000000");
    assert_eq!(Time::from_nanos(-12_250_000_000).to_string(), "-12.250000000");
    assert_eq!(Time::from_nanos(1).to_string(), "0.000000001");
    assert_eq!(Time::from_nanos(-1).to_string(), "-0.000000001");

    let min_rendered = Time::from_nanos(i64::MIN).to_string();
    assert_eq!(min_rendered, "-9223372036.854775808");
    let max_rendered = Time::from_nanos(i64::MAX).to_string();
    assert_eq!(max_rendered, "9223372036.854775807");
}

#[test]
fn time_usable_at_compile_time() {
    const START: Time = Time::from_nanos(0);
    const PERIOD: Duration = Duration::from_millis(100);
    const END_NANOS: i64 = START.nanos() + PERIOD.nanos();

    const _: () = assert!(END_NANOS == 100_000_000);
    const _: () = assert!(START.nanos() == 0);
    const _: () = assert!(START.is_zero());

    let end = START + PERIOD;
    assert_eq!(end.nanos(), 100_000_000);
    assert_eq!((end - START).nanos(), PERIOD.nanos());
}
