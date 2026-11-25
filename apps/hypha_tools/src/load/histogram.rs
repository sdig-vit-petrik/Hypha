//! A fixed-size latency histogram in nanoseconds: log-linear buckets, so its memory
//! does not grow with the number of values or the length of a run.
//!
//! Values below 64 have a bucket each. Above, each power of two is split into 64 equal
//! buckets, so a bucket is at most 1/64 (about 1.6 %) of its lower bound wide. A
//! quantile is the exclusive upper bound of the bucket that holds it, clamped to the
//! exact maximum: it never reads lower than the value it stands for, and at most about
//! 1.6 % higher. The minimum, maximum, count and sum are exact. 3,776 buckets of 8 bytes
//! each, about 30 KB per histogram.

use serde::{Deserialize, Serialize};

/// Linear buckets per power of two, as a power of two.
const SUB_BUCKET_BITS: u32 = 6;
/// Linear buckets per power of two.
const SUB_BUCKETS: u64 = 1 << SUB_BUCKET_BITS;
/// Buckets in all: 64 unit buckets, then 64 for each power of two from 2^6 to 2^63.
pub const BUCKETS: usize = 64 + 58 * 64;

/// Parts per million of the quantiles a report names.
pub const P50: u64 = 500_000;
/// The 90th percentile, in parts per million.
pub const P90: u64 = 900_000;
/// The 99th percentile, in parts per million.
pub const P99: u64 = 990_000;
/// The 99.9th percentile, in parts per million.
pub const P999: u64 = 999_000;

/// Counts of nanosecond values in fixed buckets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    counts: Vec<u64>,
    count: u64,
    sum: u64,
    min: u64,
    max: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self { counts: vec![0; BUCKETS], count: 0, sum: 0, min: u64::MAX, max: 0 }
    }
}

/// The bucket of `value`.
#[must_use]
pub fn bucket_of(value: u64) -> usize {
    if value < SUB_BUCKETS {
        return usize::try_from(value).unwrap_or(0);
    }
    // `value` >= 64, so its highest set bit is at least 6 and the shift below is in range.
    let exponent = 63_u32.saturating_sub(value.leading_zeros());
    let shift = exponent.saturating_sub(SUB_BUCKET_BITS);
    let sub = (value >> shift) & (SUB_BUCKETS - 1);
    let group = u64::from(shift);
    let index = SUB_BUCKETS.saturating_add(group.saturating_mul(SUB_BUCKETS)).saturating_add(sub);
    usize::try_from(index).unwrap_or(BUCKETS.saturating_sub(1))
}

/// The inclusive lower and exclusive upper bound of bucket `index`. The upper bound of
/// the last bucket saturates at `u64::MAX`.
#[must_use]
pub fn bucket_bounds(index: usize) -> (u64, u64) {
    let index = u64::try_from(index).unwrap_or(u64::MAX);
    if index < SUB_BUCKETS {
        return (index, index.saturating_add(1));
    }
    let above = index.saturating_sub(SUB_BUCKETS);
    let shift = above / SUB_BUCKETS;
    let sub = above % SUB_BUCKETS;
    let Ok(shift) = u32::try_from(shift) else {
        return (u64::MAX, u64::MAX);
    };
    let lower = SUB_BUCKETS.saturating_add(sub).checked_shl(shift).unwrap_or(u64::MAX);
    let width = 1_u64.checked_shl(shift).unwrap_or(u64::MAX);
    (lower, lower.saturating_add(width))
}

impl Histogram {
    /// Counts one value.
    pub fn record(&mut self, value: u64) {
        if let Some(slot) = self.counts.get_mut(bucket_of(value)) {
            *slot = slot.saturating_add(1);
        }
        self.count = self.count.saturating_add(1);
        self.sum = self.sum.saturating_add(value);
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    /// Adds every value of `other`.
    pub fn merge(&mut self, other: &Self) {
        for (slot, added) in self.counts.iter_mut().zip(&other.counts) {
            *slot = slot.saturating_add(*added);
        }
        self.count = self.count.saturating_add(other.count);
        self.sum = self.sum.saturating_add(other.sum);
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }

    /// Forgets every value.
    pub fn clear(&mut self) {
        self.counts.iter_mut().for_each(|slot| *slot = 0);
        self.count = 0;
        self.sum = 0;
        self.min = u64::MAX;
        self.max = 0;
    }

    /// How many values were counted.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// The smallest value, `None` for an empty histogram.
    #[must_use]
    pub fn min(&self) -> Option<u64> {
        (self.count > 0).then_some(self.min)
    }

    /// The largest value, `None` for an empty histogram.
    #[must_use]
    pub fn max(&self) -> Option<u64> {
        (self.count > 0).then_some(self.max)
    }

    /// The mean, rounded down; `None` for an empty histogram. The sum saturates at
    /// `u64::MAX` nanoseconds (585 years), far beyond any run.
    #[must_use]
    pub fn mean(&self) -> Option<u64> {
        self.sum.checked_div(self.count)
    }

    /// The quantile `parts_per_million` / 10^6 (a value up to 10^6) as the exclusive
    /// upper bound of the bucket that holds it, at most the exact maximum; `None` for an
    /// empty histogram.
    #[must_use]
    pub fn quantile(&self, parts_per_million: u64) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        let parts = u128::from(parts_per_million.min(1_000_000));
        // The rank of the value, 1 to count: ceil(count * parts / 10^6).
        let rank = u128::from(self.count).saturating_mul(parts).div_ceil(1_000_000).max(1);
        let mut seen: u128 = 0;
        for (index, count) in self.counts.iter().enumerate() {
            seen = seen.saturating_add(u128::from(*count));
            if seen >= rank {
                return Some(bucket_bounds(index).1.min(self.max));
            }
        }
        Some(self.max)
    }

    /// The summary a report carries, with the buckets that hold values.
    #[must_use]
    pub fn summary(&self) -> HistogramSummary {
        let buckets = self
            .counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 0)
            .map(|(index, count)| {
                let (lower, upper) = bucket_bounds(index);
                [lower, upper, *count]
            })
            .collect();
        HistogramSummary {
            count: self.count,
            min_ns: self.min(),
            max_ns: self.max(),
            mean_ns: self.mean(),
            p50_ns: self.quantile(P50),
            p90_ns: self.quantile(P90),
            p99_ns: self.quantile(P99),
            p999_ns: self.quantile(P999),
            buckets,
        }
    }
}

/// A histogram as a report carries it. The quantiles are bucket upper bounds; the
/// buckets are `[lower, upper, count]` of each bucket that holds values, at most
/// [`BUCKETS`] of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistogramSummary {
    /// Values counted.
    pub count: u64,
    /// The smallest value, exact.
    pub min_ns: Option<u64>,
    /// The largest value, exact.
    pub max_ns: Option<u64>,
    /// The mean, rounded down.
    pub mean_ns: Option<u64>,
    /// The median.
    pub p50_ns: Option<u64>,
    /// The 90th percentile.
    pub p90_ns: Option<u64>,
    /// The 99th percentile.
    pub p99_ns: Option<u64>,
    /// The 99.9th percentile.
    pub p999_ns: Option<u64>,
    /// `[lower, upper, count]` of each bucket that holds values.
    pub buckets: Vec<[u64; 3]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_falls_inside_its_bucket_and_buckets_are_at_most_a_sixteenth_wide() {
        let probes = [0, 1, 63, 64, 65, 127, 128, 129, 1000, 499_999, 500_000, 1 << 40, u64::MAX];
        for value in probes {
            let index = bucket_of(value);
            assert!(index < BUCKETS, "{value}");
            let (lower, upper) = bucket_bounds(index);
            assert!(lower <= value, "{value}: [{lower}, {upper})");
            assert!(value < upper || upper == u64::MAX, "{value}: [{lower}, {upper})");
            if lower >= 64 {
                assert!((upper - lower) * 64 <= lower, "{value}: [{lower}, {upper})");
            }
        }
        assert_eq!(bucket_of(u64::MAX), BUCKETS - 1);
    }

    #[test]
    fn buckets_are_contiguous() {
        for index in 1..BUCKETS {
            assert_eq!(bucket_bounds(index - 1).1, bucket_bounds(index).0, "bucket {index}");
        }
    }

    #[test]
    fn quantiles_are_the_upper_bound_of_the_bucket_holding_the_rank() {
        let mut histogram = Histogram::default();
        for value in 1..=100 {
            histogram.record(value);
        }
        assert_eq!(histogram.count(), 100);
        assert_eq!(histogram.min(), Some(1));
        assert_eq!(histogram.max(), Some(100));
        assert_eq!(histogram.mean(), Some(50));
        // Rank 50 is the value 50, in the bucket [50, 52).
        assert_eq!(histogram.quantile(P50), Some(bucket_bounds(bucket_of(50)).1));
        assert_eq!(histogram.quantile(P99), Some(bucket_bounds(bucket_of(99)).1));
        assert_eq!(histogram.quantile(1_000_000), Some(100), "clamped to the maximum");
        assert_eq!(histogram.quantile(0), Some(2), "rank 1 is the value 1, bucket [1, 2)");
    }

    #[test]
    fn a_quantile_never_exceeds_the_maximum_and_is_at_most_a_sixty_fourth_high() {
        let mut histogram = Histogram::default();
        histogram.record(0);
        assert_eq!(histogram.quantile(P99), Some(0));
        let mut histogram = Histogram::default();
        for value in [500_000, 500_000, 1_000_000] {
            histogram.record(value);
        }
        let median = histogram.quantile(P50).unwrap_or(0);
        assert!((500_000..=500_000 + 500_000 / 64).contains(&median), "{median}");
        assert_eq!(histogram.quantile(P99), Some(1_000_000));
    }

    #[test]
    fn an_empty_histogram_has_no_quantile_and_merge_adds_counts() {
        let empty = Histogram::default();
        assert_eq!(empty.quantile(P99), None);
        assert_eq!(empty.min(), None);
        assert_eq!(empty.summary().buckets.len(), 0);

        let mut left = Histogram::default();
        let mut right = Histogram::default();
        left.record(10);
        right.record(1_000_000);
        right.record(10);
        left.merge(&right);
        assert_eq!(left.count(), 3);
        assert_eq!(left.min(), Some(10));
        assert_eq!(left.max(), Some(1_000_000));
        let summary = left.summary();
        assert_eq!(summary.buckets.len(), 2);
        assert_eq!(summary.buckets.first().map(|bucket| bucket[2]), Some(2));
        left.clear();
        assert_eq!(left, Histogram::default());
    }

    #[test]
    fn the_sum_saturates_instead_of_overflowing() {
        let mut histogram = Histogram::default();
        histogram.record(u64::MAX);
        histogram.record(u64::MAX);
        assert_eq!(histogram.mean(), Some(u64::MAX / 2));
        assert_eq!(histogram.quantile(P50), Some(u64::MAX));
    }
}
