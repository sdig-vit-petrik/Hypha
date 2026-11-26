//! The rules of `hypha play --clock`, kept free of the bus so that each one can be
//! tested alone.
//!
//! A chunk is released only after every participant has reported a `/clock` sample
//! that the player sent at or after the chunk's send time, with a clock at or beyond
//! the chunk's boundary.

use std::time::Duration;

use hypha_node::sdk::codec::Reader;
use hypha_node::sdk::message::EncodeInto;

/// The topic on which each participant reports how far its replay has run.
pub const PROGRESS_TOPIC: &str = "/clock_progress";
/// The message type of [`PROGRESS_TOPIC`], as the generator names it.
pub const PROGRESS_DATA_TYPE: &str = Progress::TYPE_NAME;
/// The resolved definition of [`PROGRESS_DATA_TYPE`]. Its MD5 identifies the type.
pub const PROGRESS_DEFINITION: &str = "string node\nuint64 clock_nanos\nuint64 clock_sent_nanos";

/// Data time of one chunk unless `--chunk` says otherwise, in nanoseconds.
pub const DEFAULT_CHUNK_NANOS: u64 = 100_000_000;
/// How long the start barrier waits after the last new node, unless `--settle` says otherwise.
pub const DEFAULT_SETTLE: Duration = Duration::from_secs(2);
/// How long a node may report nothing before the replay stops, unless `--timeout` says otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const NANOS_PER_MILLISECOND: u64 = 1_000_000;

/// How the replay is paced on the wall clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pace {
    /// Log time runs this many times faster than wall time.
    Factor(f64),
    /// No waiting: each chunk goes out as soon as the nodes have answered the last.
    Max,
}

impl Pace {
    /// The rate factor, or `None` at [`Pace::Max`].
    #[must_use]
    pub fn factor(self) -> Option<f64> {
        match self {
            Self::Factor(factor) => Some(factor),
            Self::Max => None,
        }
    }
}

impl std::fmt::Display for Pace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Below a hundredth the two decimals of the usual form would read as zero.
            Self::Factor(factor) if *factor < 0.01 => write!(f, "{factor:e}x"),
            Self::Factor(factor) => write!(f, "{factor:.2}x"),
            Self::Max => f.write_str("max"),
        }
    }
}

/// Reads a `--rate` value: a positive factor, or `max`.
///
/// # Errors
/// Returns a message that names the value when it is neither.
pub fn parse_pace(text: &str) -> Result<Pace, String> {
    if text == "max" {
        return Ok(Pace::Max);
    }
    match text.parse::<f64>() {
        Ok(factor) if factor.is_finite() && factor > 0.0 => Ok(Pace::Factor(factor)),
        _ => Err(format!("--rate needs a positive number or 'max', got '{text}'")),
    }
}

/// Reads a duration in seconds, such as `--settle` or `--timeout`.
///
/// # Errors
/// Returns a message that names the flag when the value is not a finite, non-negative
/// number of seconds.
pub fn parse_seconds(flag: &str, text: &str) -> Result<Duration, String> {
    text.parse::<f64>()
        .ok()
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .ok_or_else(|| format!("{flag} needs a number of seconds, got '{text}'"))
}

/// The fastest rate that `+` reaches. A rate already above it is kept, never lowered.
pub const MAX_SPEED_FACTOR: f64 = 100.0;
/// The slowest rate that `-` reaches. A rate already below it is kept, never raised.
pub const MIN_SPEED_FACTOR: f64 = 0.05;

/// The factor after `+`: 1.25 times faster, capped at [`MAX_SPEED_FACTOR`].
#[must_use]
pub fn faster(factor: f64) -> f64 {
    if factor < MAX_SPEED_FACTOR { (factor * 1.25).min(MAX_SPEED_FACTOR) } else { factor }
}

/// The factor after `-`: 1.25 times slower, floored at [`MIN_SPEED_FACTOR`].
#[must_use]
pub fn slower(factor: f64) -> f64 {
    if factor > MIN_SPEED_FACTOR { (factor / 1.25).max(MIN_SPEED_FACTOR) } else { factor }
}

/// Reads a `--chunk` value in whole milliseconds, at least one, and returns nanoseconds.
///
/// # Errors
/// Returns a message that names the value when it is not a whole number of milliseconds.
pub fn parse_chunk_nanos(text: &str) -> Result<u64, String> {
    let millis = text
        .parse::<u64>()
        .ok()
        .filter(|millis| *millis > 0)
        .ok_or_else(|| format!("--chunk needs a whole number of milliseconds, got '{text}'"))?;
    millis.checked_mul(NANOS_PER_MILLISECOND).ok_or_else(|| format!("--chunk {text} is too long"))
}

/// One node's report: the `/clock` sample it has run to, and that sample's send time.
pub use hypha_node::sdk::msgs::hypha_msgs::ReplayProgress as Progress;

/// Decodes one `hypha_msgs/ReplayProgress` payload.
///
/// # Errors
/// Returns a message when the payload is truncated, has bytes left over, or names a
/// node in text that is not UTF-8.
pub fn decode_progress(bytes: &[u8]) -> Result<Progress, String> {
    let mut reader = Reader::new(bytes);
    let report =
        Progress::decode(&mut reader).map_err(|error| format!("the progress report: {error}"))?;
    if !reader.is_empty() {
        return Err(format!("{} byte(s) follow the progress report", reader.remaining()));
    }
    Ok(report)
}

/// Encodes a report as [`decode_progress`] reads it.
///
/// # Errors
/// Returns a message when the node name is longer than a `u32` can count.
pub fn encode_progress(report: &Progress) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0; report.encoded_len()];
    report.encode_into(&mut bytes).map_err(|error| format!("the progress report: {error}"))?;
    Ok(bytes)
}

/// The resolved definition of `rosgraph_msgs/Clock`, which a `/clock` publisher retains.
pub const CLOCK_DEFINITION: &str = "time clock\n";

/// The `rosgraph_msgs/Clock` payload for `log_time_nanos`.
///
/// # Errors
/// Returns a message when the seconds do not fit the `u32` of a ROS time.
pub fn clock_payload(log_time_nanos: u64) -> Result<[u8; 8], String> {
    let out_of_range = || "UTC timestamp exceeds the ROS clock range".to_string();
    let secs = u32::try_from(log_time_nanos / NANOS_PER_SECOND).map_err(|_| out_of_range())?;
    let nsecs = u32::try_from(log_time_nanos % NANOS_PER_SECOND).map_err(|_| out_of_range())?;
    let clock = hypha_node::sdk::msgs::rosgraph_msgs::Clock {
        clock: hypha_node::sdk::RosTime { secs, nsecs },
    };
    let mut bytes = [0; 8];
    clock.encode_into(&mut bytes).map_err(|_| out_of_range())?;
    Ok(bytes)
}

/// The chunk boundaries of a replay pass.
///
/// A chunk closed at `T` holds every message stamped at or before `T`: the player sends
/// `/clock = T` once they are all out, then waits for the participants before it goes on.
/// The first instant of a pass is the one exception. Its `/clock` is the start barrier,
/// sent before any data, so the messages stamped at the start follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPlan {
    chunk_nanos: u64,
    /// The earliest stamp the open chunk can hold: the start of the pass, then one
    /// nanosecond past the last close.
    opened: u64,
    /// The next boundary on the grid of `chunk_nanos` from the start.
    boundary: u64,
}

impl ChunkPlan {
    /// A plan whose first boundary is `start` plus one chunk.
    ///
    /// # Errors
    /// Returns a message when the chunk is zero or the boundary runs past the end of time.
    pub fn new(start: u64, chunk_nanos: u64) -> Result<Self, String> {
        if chunk_nanos == 0 {
            return Err("the replay chunk must be at least one millisecond".to_string());
        }
        let boundary = start
            .checked_add(chunk_nanos)
            .ok_or_else(|| "the replay runs past the end of time".to_string())?;
        Ok(Self { chunk_nanos, opened: start, boundary })
    }

    /// The next boundary on the grid: the first at or after every message given to
    /// [`Self::next_closed_before`] so far.
    #[must_use]
    pub fn boundary(&self) -> u64 {
        self.boundary
    }

    /// Closes the next boundary when it is before `log_time`, the next message to
    /// publish, and returns it. Call again until it returns `None`: a gap closes each of its
    /// boundaries, empty chunks included, one at a time.
    ///
    /// # Errors
    /// Returns a message when the boundary after it runs past the end of time.
    pub fn next_closed_before(&mut self, log_time: u64) -> Result<Option<u64>, String> {
        if log_time <= self.boundary {
            return Ok(None);
        }
        let past_the_end = || "the replay runs past the end of time".to_string();
        let closed = self.boundary;
        self.boundary = closed.checked_add(self.chunk_nanos).ok_or_else(past_the_end)?;
        self.opened = closed.checked_add(1).ok_or_else(past_the_end)?;
        Ok(Some(closed))
    }

    /// Closes the open chunk off the grid, before `log_time`, the next message to publish:
    /// one nanosecond before it, so the chunk still holds every message stamped at or before
    /// its close. Returns `None` when every message the chunk can hold is stamped
    /// `log_time`: no close comes between the messages of one instant. Call it once
    /// [`Self::next_closed_before`] has closed the boundaries before `log_time`.
    pub fn shorten_before(&mut self, log_time: u64) -> Option<u64> {
        let closed = log_time.checked_sub(1).filter(|closed| *closed >= self.opened)?;
        self.opened = log_time;
        Some(closed)
    }

    /// The boundary that ends the pass, once the boundaries up to its last message have
    /// closed: the first one after that message. It is capped at `range_end`, the exclusive
    /// end of the UTC window: the final `/clock` never passes the window.
    #[must_use]
    pub fn end_boundary(&self, range_end: Option<u64>) -> u64 {
        range_end.map_or(self.boundary, |end| self.boundary.min(end))
    }
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]
mod tests;
