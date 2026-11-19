//! Rendering a time on the Unix epoch as a UTC date.

use core::fmt;

use crate::NANOS_PER_SECOND;

const NANOS_PER_MILLISECOND: i64 = 1_000_000;
const SECONDS_PER_DAY: i64 = 86_400;
/// Days from 0000-03-01 to 1970-01-01 in the proleptic Gregorian calendar.
const DAYS_TO_EPOCH: i64 = 719_468;
const DAYS_PER_ERA: i64 = 146_097;

/// A time on the Unix epoch, shown as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
///
/// Milliseconds are truncated. Made by [`crate::Time::utc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utc {
    nanos: i64,
}

impl Utc {
    pub(crate) const fn new(nanos: i64) -> Self {
        Self { nanos }
    }
}

/// The civil date of a day count from the epoch: year, month, day.
///
/// The era is 400 years; the year starts in March, so the leap day is its last day.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days.wrapping_add(DAYS_TO_EPOCH);
    let era = shifted.div_euclid(DAYS_PER_ERA);
    let day_of_era = shifted.rem_euclid(DAYS_PER_ERA);
    let year_of_era = day_of_era
        .wrapping_sub(day_of_era.wrapping_div(1_460))
        .wrapping_add(day_of_era.wrapping_div(36_524))
        .wrapping_sub(day_of_era.wrapping_div(146_096))
        .wrapping_div(365);
    let day_of_year = day_of_era.wrapping_sub(
        year_of_era
            .wrapping_mul(365)
            .wrapping_add(year_of_era.wrapping_div(4))
            .wrapping_sub(year_of_era.wrapping_div(100)),
    );
    let month_from_march = day_of_year.wrapping_mul(5).wrapping_add(2).wrapping_div(153);
    let day = day_of_year
        .wrapping_sub(month_from_march.wrapping_mul(153).wrapping_add(2).wrapping_div(5))
        .wrapping_add(1);
    let month = if month_from_march < 10 {
        month_from_march.wrapping_add(3)
    } else {
        month_from_march.wrapping_sub(9)
    };
    let march_based_year = year_of_era.wrapping_add(era.wrapping_mul(400));
    let year = if month <= 2 { march_based_year.wrapping_add(1) } else { march_based_year };
    (year, month, day)
}

impl fmt::Display for Utc {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self.nanos.div_euclid(NANOS_PER_SECOND);
        let millis = self.nanos.rem_euclid(NANOS_PER_SECOND).wrapping_div(NANOS_PER_MILLISECOND);
        let (year, month, day) = civil_from_days(seconds.div_euclid(SECONDS_PER_DAY));
        let second_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
        let hour = second_of_day.wrapping_div(3_600);
        let minute = second_of_day.rem_euclid(3_600).wrapping_div(60);
        let second = second_of_day.rem_euclid(60);
        write!(out, "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
    }
}
