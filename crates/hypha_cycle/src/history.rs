//! Bounded owned sample history shared by independent cycle bindings.

use std::collections::VecDeque;
use std::ops::Deref;
use std::sync::Arc;

/// Reusable serialized message buffer that exposes only its immutable slice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SamplePayload(Vec<u8>);

impl AsRef<[u8]> for SamplePayload {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Deref for SamplePayload {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

/// Which frame of which publisher a sample came from. Zero for a sample pushed
/// without a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SampleOrigin {
    /// The publisher instance that sent the frame.
    pub publisher_id: u64,
    /// The send sequence of that publisher. It wraps.
    pub sequence: u64,
}

/// One immutable serialized message retained below the ABI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySample {
    /// Sequence used by independent queued cursors.
    pub sequence: u64,
    /// ROS wire payload, without the hypha frame header.
    pub payload: SamplePayload,
    /// Host monotonic send timestamp from the validated frame.
    pub transport_sent_nanos: u64,
    /// Framework-time send timestamp from the validated frame (sim time on a sim-time node).
    pub framework_sent_nanos: u64,
    /// The publisher and its send sequence, from the validated frame.
    pub origin: SampleOrigin,
}

/// Current cumulative history outcomes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HistoryStats {
    /// Total samples accepted by this history.
    pub accepted: u64,
    /// Accepted samples a reader reported as undecodable (see [`CycleHistory::reject`]).
    pub rejected: u64,
}

/// Count-bounded owned sample history.
#[derive(Debug, Clone)]
pub struct CycleHistory {
    samples: VecDeque<Arc<HistorySample>>,
    max_samples: usize,
    next_sequence: u64,
    stats: HistoryStats,
    /// Descending adjacent framework timestamps in the retained history; nonzero disables
    /// timestamp binary search.
    timestamp_inversions: usize,
    /// The retained samples reported as undecodable, by sequence. Pruned on eviction.
    rejected: Vec<u64>,
    /// The newest framework send stamp of an evicted sample.
    newest_evicted_sent_nanos: Option<u64>,
    /// The sequence of the newest evicted sample a reader had rejected: eviction prunes
    /// `rejected`, and a binding still has to know whether a sample it relied on was one.
    newest_evicted_rejected: Option<u64>,
}

impl CycleHistory {
    /// Create a history that retains at most `max_samples` samples.
    ///
    /// Returns `None` when `max_samples` is zero.
    #[must_use]
    pub fn new(max_samples: usize) -> Option<Self> {
        Self::with_initial_sequence(max_samples, 0)
    }

    /// Create a history whose next assigned sequence is `initial_sequence`.
    ///
    /// Also the seam used to exercise cursor wraparound.
    #[must_use]
    pub fn with_initial_sequence(max_samples: usize, initial_sequence: u64) -> Option<Self> {
        if max_samples == 0 {
            return None;
        }
        Some(Self {
            samples: VecDeque::new(),
            max_samples,
            next_sequence: initial_sequence,
            stats: HistoryStats::default(),
            timestamp_inversions: 0,
            rejected: Vec::new(),
            newest_evicted_sent_nanos: None,
            newest_evicted_rejected: None,
        })
    }

    /// Ensure the history can retain at least `min_samples`.
    pub fn ensure_capacity(&mut self, min_samples: usize) {
        if min_samples > self.max_samples {
            self.max_samples = min_samples;
        }
    }

    /// Copy a payload into the history, evicting the oldest sample when the history
    /// is full, and return the sequence it assigned.
    pub fn push(
        &mut self,
        payload: &[u8],
        transport_sent_nanos: u64,
        framework_sent_nanos: u64,
    ) -> u64 {
        self.push_from(payload, transport_sent_nanos, framework_sent_nanos, SampleOrigin::default())
    }

    /// [`CycleHistory::push`] for a payload of the frame `origin` names.
    pub fn push_from(
        &mut self,
        payload: &[u8],
        transport_sent_nanos: u64,
        framework_sent_nanos: u64,
        origin: SampleOrigin,
    ) -> u64 {
        let sequence = self.next_sequence;

        let mut reused: Option<Arc<HistorySample>> = None;
        loop {
            if self.samples.len() < self.max_samples {
                break;
            }
            let Some(mut oldest) = self.samples.pop_front() else {
                break;
            };
            let evicted = oldest.sequence;
            if self.rejected.contains(&evicted) {
                self.newest_evicted_rejected = Some(evicted);
            }
            self.rejected.retain(|sequence| *sequence != evicted);
            let evicted_sent = oldest.framework_sent_nanos;
            let newest = self
                .newest_evicted_sent_nanos
                .map_or(evicted_sent, |newest| newest.max(evicted_sent));
            self.newest_evicted_sent_nanos = Some(newest);
            if self
                .samples
                .front()
                .is_some_and(|next| oldest.framework_sent_nanos > next.framework_sent_nanos)
            {
                self.timestamp_inversions = self.timestamp_inversions.saturating_sub(1);
            }
            if Arc::get_mut(&mut oldest).is_some() {
                let keep = reused.as_ref().is_none_or(|current| {
                    oldest.payload.0.capacity() > current.payload.0.capacity()
                });
                if keep {
                    reused = Some(oldest);
                }
            }
        }

        let sample = write_sample(
            reused,
            sequence,
            payload,
            transport_sent_nanos,
            framework_sent_nanos,
            origin,
        );

        if self.samples.back().is_some_and(|last| last.framework_sent_nanos > framework_sent_nanos)
        {
            self.timestamp_inversions = self.timestamp_inversions.saturating_add(1);
        }
        self.samples.push_back(sample);
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.stats.accepted = self.stats.accepted.wrapping_add(1);
        sequence
    }

    /// The newest framework send stamp of a sample the history evicted.
    #[must_use]
    pub fn newest_evicted_sent_nanos(&self) -> Option<u64> {
        self.newest_evicted_sent_nanos
    }

    /// Count `count` samples as accepted that the caller did not store, because a newer
    /// sample replaces them for every reader. Only for a history no queued cursor reads:
    /// it leaves a gap in the retained sequences.
    pub fn count_unstored(&mut self, count: u64) {
        self.stats.accepted = self.stats.accepted.wrapping_add(count);
        self.next_sequence = self.next_sequence.wrapping_add(count);
    }

    /// Mark the retained sample `sequence` as undecodable: no binding selects it or
    /// judges freshness by it again. The sample stays in the history. Returns false when
    /// no retained sample has that sequence or it was rejected before.
    pub fn reject(&mut self, sequence: u64) -> bool {
        let retained = self.samples.iter().any(|sample| sample.sequence == sequence);
        let known = self.rejected.contains(&sequence);
        let newly_rejected = retained && !known;
        if newly_rejected {
            self.rejected.push(sequence);
            self.stats.rejected = self.stats.rejected.wrapping_add(1);
        }
        newly_rejected
    }

    /// The sequence of the newest evicted sample that was reported as undecodable.
    pub(crate) const fn newest_evicted_rejected(&self) -> Option<u64> {
        self.newest_evicted_rejected
    }

    /// Whether the retained sample `sequence` was reported as undecodable.
    #[must_use]
    pub fn is_rejected(&self, sequence: u64) -> bool {
        self.rejected.contains(&sequence)
    }

    /// Retained samples in arrival order.
    #[must_use]
    pub fn samples(&self) -> &VecDeque<Arc<HistorySample>> {
        &self.samples
    }

    /// The next sequence to assign.
    #[must_use]
    pub const fn high_water_next(&self) -> u64 {
        self.next_sequence
    }

    /// Current cumulative counters.
    #[must_use]
    pub const fn stats(&self) -> HistoryStats {
        self.stats
    }

    /// Offset of a queued cursor, clamped to the retained window.
    pub(crate) fn cursor_offset(&self, cursor: u64) -> usize {
        let Some(oldest) = self.samples.front() else {
            return 0;
        };
        let offset = cursor.wrapping_sub(oldest.sequence);
        if offset >= (1_u64 << 63) {
            return 0;
        }
        usize::try_from(offset).unwrap_or(usize::MAX).min(self.samples.len())
    }

    /// The exclusive end of the prefix of samples whose framework send time `admits`, when
    /// timestamps are ordered.
    pub(crate) fn replay_end(&self, admits: impl Fn(u64) -> bool) -> Option<usize> {
        (self.timestamp_inversions == 0)
            .then(|| self.samples.partition_point(|sample| admits(sample.framework_sent_nanos)))
    }

    /// The retained samples stamped more than `bound_nanos` after `cut_nanos`; a high
    /// count says the publisher is on another time base.
    #[must_use]
    pub fn stamped_beyond(&self, cut_nanos: u64, bound_nanos: u64) -> usize {
        let limit = cut_nanos.saturating_add(bound_nanos);
        if self.timestamp_inversions == 0 {
            let end = self.samples.partition_point(|sample| sample.framework_sent_nanos <= limit);
            return self.samples.len().saturating_sub(end);
        }
        self.samples.iter().filter(|sample| sample.framework_sent_nanos > limit).count()
    }
}

fn write_sample(
    reused: Option<Arc<HistorySample>>,
    sequence: u64,
    payload: &[u8],
    transport_sent_nanos: u64,
    framework_sent_nanos: u64,
    origin: SampleOrigin,
) -> Arc<HistorySample> {
    if let Some(mut arc) = reused
        && let Some(sample) = Arc::get_mut(&mut arc)
    {
        sample.sequence = sequence;
        sample.payload.0.clear();
        sample.payload.0.extend_from_slice(payload);
        sample.transport_sent_nanos = transport_sent_nanos;
        sample.framework_sent_nanos = framework_sent_nanos;
        sample.origin = origin;
        return arc;
    }
    Arc::new(HistorySample {
        sequence,
        payload: SamplePayload(payload.to_vec()),
        transport_sent_nanos,
        framework_sent_nanos,
        origin,
    })
}
