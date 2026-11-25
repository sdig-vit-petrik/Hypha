//! A time series of bounded size for any run length: when it is full, each pair of
//! neighbouring points merges into one and every later point spans twice as many
//! sample intervals.

use serde::{Deserialize, Serialize};

/// A point that two neighbouring points can merge into.
pub trait Merge {
    /// Folds `later`, the point that follows this one, into this one.
    fn merge(&mut self, later: &Self);
}

/// At most `capacity` points, each standing for `stride` sample intervals.
#[derive(Debug, Clone, PartialEq)]
pub struct Series<T> {
    capacity: usize,
    stride: u64,
    points: Vec<T>,
    /// The point being built and how many intervals it holds, fewer than `stride`.
    pending: Option<(T, u64)>,
}

impl<T: Merge + Clone> Series<T> {
    /// An empty series of at most `capacity` points, at least 2.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2);
        Self { capacity, stride: 1, points: Vec::with_capacity(capacity), pending: None }
    }

    /// Adds the point of one sample interval.
    pub fn push(&mut self, point: T) {
        let (point, held) = match self.pending.take() {
            Some((mut pending, held)) => {
                pending.merge(&point);
                (pending, held.saturating_add(1))
            }
            None => (point, 1),
        };
        if held < self.stride {
            self.pending = Some((point, held));
            return;
        }
        if self.points.len() >= self.capacity {
            // The point spans the old stride, half the new one: it goes on collecting.
            self.halve();
            self.pending = Some((point, held));
            return;
        }
        self.points.push(point);
    }

    /// Merges each pair of points and doubles the stride.
    fn halve(&mut self) {
        let mut merged = Vec::with_capacity(self.capacity);
        let mut points = std::mem::take(&mut self.points).into_iter();
        while let Some(mut first) = points.next() {
            if let Some(second) = points.next() {
                first.merge(&second);
            }
            merged.push(first);
        }
        self.points = merged;
        self.stride = self.stride.saturating_mul(2);
    }

    /// The sample intervals one stored point stands for.
    #[must_use]
    pub fn stride(&self) -> u64 {
        self.stride
    }

    /// The stored points, oldest first, then the point being built.
    #[must_use]
    pub fn points(&self) -> Vec<T> {
        let mut points = self.points.clone();
        points.extend(self.pending.as_ref().map(|(point, _)| point.clone()));
        points
    }
}

/// The larger of two optional values; a missing one gives way.
#[must_use]
pub fn larger(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (one, None) | (None, one) => one,
    }
}

/// The memory of one process at one point.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPoint {
    /// Resident bytes; `None` when the process could not be read.
    pub rss: Option<u64>,
    /// Resident shared-memory bytes; `None` where the system does not tell them.
    pub shmem: Option<u64>,
}

/// One point of a run: the largest values its span saw.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeriesPoint {
    /// Milliseconds from the start of publishing to the span's first sample; the first
    /// point is taken when publishing starts.
    pub t_ms: i64,
    /// The memory of each process, in the order of the report's `processes`.
    pub memory: Vec<MemoryPoint>,
    /// The written shared memory of the run's domain; `None` where it is not told.
    pub domain_shm: Option<u64>,
    /// The 99th percentile latency of each subscriber over the span, in the order of
    /// the report's subscribers; `None` for a span without a sample in the window.
    pub p99_ns: Vec<Option<u64>>,
}

impl Merge for SeriesPoint {
    fn merge(&mut self, later: &Self) {
        for (memory, next) in self.memory.iter_mut().zip(&later.memory) {
            memory.rss = larger(memory.rss, next.rss);
            memory.shmem = larger(memory.shmem, next.shmem);
        }
        self.domain_shm = larger(self.domain_shm, later.domain_shm);
        for (p99, next) in self.p99_ns.iter_mut().zip(&later.p99_ns) {
            *p99 = larger(*p99, *next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Peak(u64);

    impl Merge for Peak {
        fn merge(&mut self, later: &Self) {
            self.0 = self.0.max(later.0);
        }
    }

    #[test]
    fn the_series_stays_within_its_capacity_for_any_number_of_points() {
        let mut series = Series::new(8);
        for value in 0..100_000_u64 {
            series.push(Peak(value));
            assert!(series.points().len() <= 9, "at most the capacity plus the pending point");
        }
        let points = series.points();
        assert!(points.len() >= 4, "decimation keeps at least half the capacity");
        assert_eq!(points.last(), Some(&Peak(99_999)), "the newest value is kept");
        assert!(series.stride() >= 100_000 / 8, "{}", series.stride());
    }

    #[test]
    fn decimation_keeps_the_largest_value_of_each_span() {
        let mut series = Series::new(4);
        for value in [1, 9, 2, 3, 7, 4, 5, 6] {
            series.push(Peak(value));
        }
        // Four points; the fifth halves them to [9, 3] and then spans two intervals.
        let points = series.points();
        assert_eq!(points, vec![Peak(9), Peak(3), Peak(7), Peak(6)]);
        assert!(points.contains(&Peak(9)), "{points:?}");
        assert!(points.iter().any(|peak| peak.0 >= 7), "{points:?}");
        assert_eq!(series.stride(), 2);
    }

    #[test]
    fn a_point_merges_the_larger_memory_and_latency() {
        let mut first = SeriesPoint {
            t_ms: 0,
            memory: vec![MemoryPoint { rss: Some(10), shmem: None }],
            domain_shm: Some(5),
            p99_ns: vec![None, Some(3)],
        };
        let second = SeriesPoint {
            t_ms: 1000,
            memory: vec![MemoryPoint { rss: Some(4), shmem: Some(2) }],
            domain_shm: None,
            p99_ns: vec![Some(8), Some(1)],
        };
        first.merge(&second);
        assert_eq!(first.t_ms, 0, "a merged point starts where its span starts");
        assert_eq!(first.memory, vec![MemoryPoint { rss: Some(10), shmem: Some(2) }]);
        assert_eq!(first.domain_shm, Some(5));
        assert_eq!(first.p99_ns, vec![Some(8), Some(3)]);
    }
}
