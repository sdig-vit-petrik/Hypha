//! Operator UTC times and time-window validation.
#![allow(clippy::expect_used)]

use hypha_tools::utc_time::{UtcRange, parse_utc_nanos};

#[test]
fn timezone_offsets_and_nanoseconds_select_the_same_utc_instant() {
    let epoch = 1_791_460_800_000_000_000;
    assert_eq!(parse_utc_nanos("2026-10-08T12:00:00Z"), Ok(epoch));
    assert_eq!(parse_utc_nanos("2026-10-08T14:00:00+02:00"), Ok(epoch));
    assert_eq!(parse_utc_nanos("2026-10-08T07:00:00-05:00"), Ok(epoch));
    assert_eq!(parse_utc_nanos("2026-10-08T12:00:00.123456789Z"), Ok(1_791_460_800_123_456_789));
    assert_eq!(parse_utc_nanos("1969-12-31T23:00:00-01:00"), Ok(0));
    assert_eq!(parse_utc_nanos("2000-02-29T00:00:00Z"), Ok(951_782_400_000_000_000));
}

#[test]
fn ambiguous_and_unrepresentable_customer_times_are_refused() {
    for invalid in [
        "2026-10-08T12:00:00",
        "2026-10-08",
        "12:00:00Z",
        "2026-02-29T00:00:00Z",
        "2024-02-30T00:00:00Z",
        "1900-02-29T00:00:00Z",
        "2026-12-31T23:59:60Z",
        "1969-12-31T23:59:59Z",
        "9999-12-31T23:59:59Z",
        "not a time",
    ] {
        assert!(parse_utc_nanos(invalid).is_err(), "must refuse {invalid}");
    }
}

#[test]
fn utc_windows_are_inclusive_at_start_and_exclusive_at_end() {
    let range = UtcRange::new(Some(10), Some(20)).expect("window");
    assert!(!range.contains(9));
    assert!(range.contains(10));
    assert!(range.contains(19));
    assert!(!range.contains(20));
    assert!(UtcRange::new(Some(20), Some(10)).is_err());
    assert!(UtcRange::new(Some(10), Some(10)).is_err());
    assert!(UtcRange::default().contains(0));
}
