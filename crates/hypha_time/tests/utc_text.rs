//! A time on the Unix epoch prints as a UTC date with milliseconds.

use hypha_time::Time;

fn utc(nanos: i64) -> String {
    format!("{}", Time::from_nanos(nanos).utc())
}

#[test]
fn the_epoch_prints_as_its_date() {
    assert_eq!(utc(0), "1970-01-01T00:00:00.000Z");
}

#[test]
fn a_time_before_the_epoch_prints_the_day_before() {
    assert_eq!(utc(-1), "1969-12-31T23:59:59.999Z");
}

#[test]
fn a_leap_day_prints_as_february_the_29th() {
    assert_eq!(utc(951_782_400_000_000_000), "2000-02-29T00:00:00.000Z");
    assert_eq!(utc(1_709_164_799_999_999_999), "2024-02-28T23:59:59.999Z");
}

#[test]
fn milliseconds_are_truncated_not_rounded() {
    assert_eq!(utc(1_791_234_567_123_456_789), "2026-10-05T21:09:27.123Z");
}

#[test]
fn the_ends_of_the_range_print() {
    assert_eq!(utc(i64::MAX), "2262-04-11T23:47:16.854Z");
    assert_eq!(utc(i64::MIN), "1677-09-21T00:12:43.145Z");
}
