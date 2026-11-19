//! The two time values a message definition can name.
//! Both keep the two-part wire form, so a value reads back bit for bit.

use crate::codec::{CodecError, Reader};
use crate::slice_writer::SliceWriter;

const NANOS_PER_SECOND: u32 = 1_000_000_000;

/// A point in time: whole seconds and a nanosecond remainder.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RosTime {
    /// Whole seconds.
    pub secs: u32,
    /// Nanoseconds within the second.
    pub nsecs: u32,
}

impl RosTime {
    /// A point in time from its two parts.
    #[must_use]
    pub const fn new(secs: u32, nsecs: u32) -> Self {
        Self { secs, nsecs }
    }

    /// Appends the value to `writer`.
    ///
    /// # Errors
    /// Returns `Truncated` when fewer than eight bytes remain.
    pub fn encode(self, writer: &mut SliceWriter<'_>) -> Result<(), CodecError> {
        writer.write_time(self.secs, self.nsecs)
    }

    /// Reads one value from `reader`.
    ///
    /// # Errors
    /// Returns an error when fewer than eight bytes remain.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, CodecError> {
        let (secs, nsecs) = reader.read_time()?;
        Ok(Self { secs, nsecs })
    }

    /// Reads a complete `rosgraph_msgs/Clock` payload.
    ///
    /// Returns `None` unless the payload is exactly one `time` and its
    /// nanoseconds are below one second.
    #[must_use]
    pub fn decode_clock(payload: &[u8]) -> Option<Self> {
        let mut reader = Reader::new(payload);
        let time = Self::decode(&mut reader).ok()?;
        let is_complete_clock = reader.is_empty() && time.nsecs < NANOS_PER_SECOND;
        is_complete_clock.then_some(time)
    }

    /// The value as nanoseconds since its epoch.
    #[must_use]
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "u32::MAX seconds plus u32::MAX nanoseconds stays below u64::MAX"
    )]
    pub fn as_nanos(self) -> u64 {
        u64::from(self.secs) * u64::from(NANOS_PER_SECOND) + u64::from(self.nsecs)
    }
}

/// A span of time: whole seconds and a nanosecond remainder, both signed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RosDuration {
    /// Whole seconds. May be negative.
    pub secs: i32,
    /// Nanoseconds within the second. May be negative.
    pub nsecs: i32,
}

impl RosDuration {
    /// A span from its two parts.
    #[must_use]
    pub const fn new(secs: i32, nsecs: i32) -> Self {
        Self { secs, nsecs }
    }

    /// Appends the value to `writer`.
    ///
    /// # Errors
    /// Returns `Truncated` when fewer than eight bytes remain.
    pub fn encode(self, writer: &mut SliceWriter<'_>) -> Result<(), CodecError> {
        writer.write_duration(self.secs, self.nsecs)
    }

    /// Reads one value from `reader`.
    ///
    /// # Errors
    /// Returns an error when fewer than eight bytes remain.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, CodecError> {
        let (secs, nsecs) = reader.read_duration()?;
        Ok(Self { secs, nsecs })
    }
}
