//! Absolute UTC times and recording windows.

use toml::value::{Datetime, Offset};

/// An inclusive UTC start and exclusive UTC end. An absent bound is unrestricted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UtcRange {
    start: Option<u64>,
    end: Option<u64>,
}

impl UtcRange {
    /// Construct a nonempty absolute time window.
    ///
    /// # Errors
    /// Refuses an end at or before the start.
    pub fn new(start: Option<u64>, end: Option<u64>) -> Result<Self, &'static str> {
        if let (Some(start), Some(end)) = (start, end)
            && end <= start
        {
            return Err("--end-utc must be after --start-utc");
        }
        Ok(Self { start, end })
    }

    /// Inclusive UTC start, if set.
    #[must_use]
    pub const fn start(self) -> Option<u64> {
        self.start
    }

    /// Exclusive UTC end, if set.
    #[must_use]
    pub const fn end(self) -> Option<u64> {
        self.end
    }

    /// Whether a recording timestamp lies in this window.
    #[must_use]
    pub fn contains(self, time: u64) -> bool {
        self.start.is_none_or(|start| time >= start) && self.end.is_none_or(|end| time < end)
    }
}

/// Parse a timestamp with an explicit timezone into UTC Unix-epoch nanoseconds.
///
/// # Errors
/// Refuses local date-times, leap seconds, pre-epoch results and overflow.
pub fn parse_utc_nanos(text: &str) -> Result<u64, &'static str> {
    let value = text
        .parse::<Datetime>()
        .map_err(|_| "expected a timestamp such as 2026-10-08T12:30:00Z")?;
    let date = value.date.ok_or("UTC timestamp needs a date")?;
    let time = value.time.ok_or("UTC timestamp needs a time")?;
    let offset = match value.offset.ok_or("UTC timestamp needs Z or an explicit timezone offset")? {
        Offset::Z => 0,
        Offset::Custom { minutes } => i128::from(minutes),
    };
    if time.second >= 60 {
        return Err("leap seconds cannot be represented in Unix time");
    }
    let year = i128::from(date.year);
    let previous = year.checked_sub(1).ok_or("date overflow")?;
    let days_before_year = previous
        .checked_mul(365)
        .and_then(|days| days.checked_add(previous / 4))
        .and_then(|days| days.checked_sub(previous / 100))
        .and_then(|days| days.checked_add(previous / 400))
        .ok_or("date overflow")?;
    let month_index = usize::from(date.month.checked_sub(1).ok_or("invalid month")?);
    let month_days = *[0_i128, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334]
        .get(month_index)
        .ok_or("invalid month")?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_length = *[31_u8, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        .get(month_index)
        .ok_or("invalid month")?;
    let month_length =
        month_length.checked_add(u8::from(leap && date.month == 2)).ok_or("invalid month")?;
    if date.day > month_length {
        return Err("invalid calendar date");
    }
    let leap_day = i128::from(leap && date.month > 2);
    let day = i128::from(date.day.checked_sub(1).ok_or("invalid day")?);
    let days = days_before_year
        .checked_sub(719_162)
        .and_then(|days| days.checked_add(month_days))
        .and_then(|days| days.checked_add(leap_day))
        .and_then(|days| days.checked_add(day))
        .ok_or("date overflow")?;
    let seconds = days
        .checked_mul(86_400)
        .and_then(|seconds| seconds.checked_add(i128::from(time.hour).checked_mul(3600)?))
        .and_then(|seconds| seconds.checked_add(i128::from(time.minute).checked_mul(60)?))
        .and_then(|seconds| seconds.checked_add(i128::from(time.second)))
        .and_then(|seconds| seconds.checked_sub(offset.checked_mul(60)?))
        .ok_or("UTC timestamp overflow")?;
    let nanos = seconds
        .checked_mul(1_000_000_000)
        .and_then(|nanos| nanos.checked_add(i128::from(time.nanosecond)))
        .ok_or("UTC timestamp overflow")?;
    u64::try_from(nanos).map_err(|_| "UTC timestamp precedes 1970 or exceeds nanosecond range")
}
