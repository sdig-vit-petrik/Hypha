//! Rendering a nanosecond count as seconds with nine fractional digits.

use core::fmt::{self, Write as _};

/// Nanoseconds in one second, as a literal so the divisions are provably not by zero.
const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// Write `nanos` as signed seconds with exactly nine fractional digits.
///
/// [`i64::MIN`] renders instead of overflowing.
pub(crate) fn format_nanos(nanos: i64, out: &mut fmt::Formatter<'_>) -> fmt::Result {
    let magnitude = nanos.unsigned_abs();
    let whole = magnitude / NANOS_PER_SECOND;
    let fraction = magnitude % NANOS_PER_SECOND;

    if nanos < 0 {
        out.write_char('-')?;
    }
    write!(out, "{whole}.{fraction:09}")
}
