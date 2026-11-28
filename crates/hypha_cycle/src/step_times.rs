//! How long a cycle's steps take, as a distribution in constant memory.
//!
//! A step's duration goes into a log-linear histogram: values below 16 ns have a bucket
//! each, and every power of two above is split into 16 equal buckets, so a bucket is at
//! most 1/16 of its lower bound wide. A quantile is the inclusive upper bound of the
//! bucket that holds it, at most the exact maximum: it never reads below the value it
//! stands for and at most 1/16 above. Steps of 2^40 ns (18 minutes) and longer share the
//! last bucket; the maximum stays exact. Recording is an index computation and three
//! increments, and allocates nothing: the whole state is three arrays of 592 counters.
//!
//! Two scopes are kept: every step since the cycle was committed, and the steps of the
//! last [`StepTimes::WINDOW_NANOS`] to twice that, so one slow start-up step does not own
//! the maximum for the life of the process.

/// Linear buckets per power of two, as a power of two.
const SUB_BITS: u32 = 4;
/// Linear buckets per power of two.
const SUB: u64 = 1 << SUB_BITS;
/// The last bucket takes every value from `2^TOP_BITS` ns.
const TOP_BITS: u32 = 40;
/// 16 unit buckets, then 16 for each power of two from 2^4 up to 2^39.
const BUCKETS: usize = 592;

/// The median, the 90th and the 99th percentile, in parts per million.
const QUANTILES: [u64; 3] = [500_000, 900_000, 990_000];

/// The bucket of `value`.
fn bucket_of(value: u64) -> usize {
    let value = value.min((1 << TOP_BITS) - 1);
    if value < SUB {
        return usize::try_from(value).unwrap_or(0);
    }
    // `value` >= 16, so its highest set bit is at least 4 and the shift is in range.
    let shift = 63_u32.saturating_sub(value.leading_zeros()).saturating_sub(SUB_BITS);
    let sub = (value >> shift) & (SUB - 1);
    let index = SUB.saturating_add(u64::from(shift).saturating_mul(SUB)).saturating_add(sub);
    usize::try_from(index).unwrap_or(BUCKETS.saturating_sub(1))
}

/// The largest value of bucket `index`; the last bucket has no limit.
fn bucket_top(index: usize) -> u64 {
    if index >= BUCKETS.saturating_sub(1) {
        return u64::MAX;
    }
    let index = u64::try_from(index).unwrap_or(u64::MAX);
    if index < SUB {
        return index;
    }
    let above = index.saturating_sub(SUB);
    let shift = u32::try_from(above / SUB).unwrap_or(u32::MAX);
    let lower = SUB.saturating_add(above % SUB).checked_shl(shift).unwrap_or(u64::MAX);
    lower.saturating_add((1_u64.checked_shl(shift).unwrap_or(u64::MAX)).saturating_sub(1))
}

/// Counts of step durations in fixed buckets, and the exact largest.
#[derive(Clone)]
struct Histogram {
    counts: [u64; BUCKETS],
    count: u64,
    max: u64,
}

impl Histogram {
    const EMPTY: Self = Self { counts: [0; BUCKETS], count: 0, max: 0 };

    fn record(&mut self, value: u64) {
        if let Some(slot) = self.counts.get_mut(bucket_of(value)) {
            *slot = slot.saturating_add(1);
        }
        self.count = self.count.saturating_add(1);
        self.max = self.max.max(value);
    }
}

/// The quantiles `parts_per_million` / 10^6, in ascending order, of the values of all
/// `parts` together, in one pass over the buckets; zero for each when they hold no value.
fn quantiles<const N: usize>(parts: &[&Histogram], parts_per_million: [u64; N]) -> [u64; N] {
    let total: u64 = parts.iter().map(|part| part.count).sum();
    let largest = parts.iter().map(|part| part.max).max().unwrap_or(0);
    let mut found = [0; N];
    let mut next = 0;
    let mut seen = 0_u64;
    for index in 0..BUCKETS {
        seen = seen.saturating_add(
            parts.iter().map(|part| part.counts.get(index).copied().unwrap_or(0)).sum(),
        );
        while let Some(ppm) = parts_per_million.get(next) {
            if seen < total.saturating_mul(*ppm).div_ceil(1_000_000).max(1) {
                break;
            }
            if let Some(slot) = found.get_mut(next) {
                *slot = bucket_top(index).min(largest);
            }
            next = next.saturating_add(1);
        }
    }
    found
}

/// What `~/state` reports of the step durations of one cycle, in nanoseconds of host time.
/// Every field is zero before the first step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepSummary {
    /// Median of every step since the cycle was committed.
    pub p50_nanos: u64,
    /// 90th percentile of every step since the commit.
    pub p90_nanos: u64,
    /// 99th percentile of every step since the commit.
    pub p99_nanos: u64,
    /// Median of the recent steps: the last [`StepTimes::WINDOW_NANOS`] to twice that.
    pub recent_p50_nanos: u64,
    /// 90th percentile of the recent steps.
    pub recent_p90_nanos: u64,
    /// 99th percentile of the recent steps.
    pub recent_p99_nanos: u64,
    /// The longest recent step, exact.
    pub recent_max_nanos: u64,
}

/// The step durations of one cycle: every step since the commit and a recent window.
#[derive(Clone)]
pub struct StepTimes {
    total: Histogram,
    current: Histogram,
    previous: Histogram,
    /// The host time at which `current` began.
    started_nanos: u64,
}

impl Default for StepTimes {
    fn default() -> Self {
        Self {
            total: Histogram::EMPTY,
            current: Histogram::EMPTY,
            previous: Histogram::EMPTY,
            started_nanos: 0,
        }
    }
}

impl StepTimes {
    /// The window turns over when this much host time has passed since it began: the recent
    /// quantiles cover the last 10 to 20 seconds.
    pub const WINDOW_NANOS: u64 = 10_000_000_000;

    /// Counts a step of `duration_nanos` that ended at the host monotonic time `now_nanos`.
    pub fn record(&mut self, duration_nanos: u64, now_nanos: u64) {
        let age = now_nanos.saturating_sub(self.started_nanos);
        if age >= Self::WINDOW_NANOS {
            std::mem::swap(&mut self.current, &mut self.previous);
            self.current = Histogram::EMPTY;
            if age >= Self::WINDOW_NANOS.saturating_mul(2) {
                self.previous = Histogram::EMPTY;
            }
            self.started_nanos = now_nanos;
        }
        self.total.record(duration_nanos);
        self.current.record(duration_nanos);
    }

    /// The quantiles and the recent maximum now.
    #[must_use]
    pub fn summary(&self) -> StepSummary {
        let [p50_nanos, p90_nanos, p99_nanos] = quantiles(&[&self.total], QUANTILES);
        let [recent_p50_nanos, recent_p90_nanos, recent_p99_nanos] =
            quantiles(&[&self.current, &self.previous], QUANTILES);
        StepSummary {
            p50_nanos,
            p90_nanos,
            p99_nanos,
            recent_p50_nanos,
            recent_p90_nanos,
            recent_p99_nanos,
            recent_max_nanos: self.current.max.max(self.previous.max),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_lies_in_its_bucket_at_most_a_sixteenth_below_its_top() {
        assert_eq!(bucket_of(u64::MAX), BUCKETS.saturating_sub(1));
        assert_eq!(bucket_top(BUCKETS.saturating_sub(1)), u64::MAX);
        assert_eq!(bucket_top(BUCKETS.saturating_sub(2)), (1 << TOP_BITS) - (1 << 35) - 1);
        for exponent in 0..TOP_BITS.saturating_sub(1) {
            for offset in [0_u64, 1, 7, 100] {
                let value = (1_u64 << exponent).saturating_add(offset);
                let top = bucket_top(bucket_of(value));
                assert!(top >= value, "{value} above the top {top} of its bucket");
                assert!(
                    top.saturating_sub(value) <= value / 16,
                    "{value}: its bucket reaches {top}"
                );
            }
        }
    }
}
