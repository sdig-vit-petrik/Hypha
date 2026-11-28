//! Bounded owned sample history shared by independent cycle bindings.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

mod pins;
pub use pins::{PinLedger, PinReaders};

/// The bytes of one sample a transport holds where they arrived, read there.
pub trait PinnedBytes: Send {
    /// The message bytes.
    fn bytes(&self) -> &[u8];
}

/// Where the bytes of a pinned sample are: in place, copied out, or given back unread.
enum PinState {
    InPlace(Box<dyn PinnedBytes>),
    Copied(Vec<u8>),
    Released,
}

/// The bytes of one sample read in place, in the transport's slot, until the transport
/// needs the slot back: then they are copied ([`PinnedPayload::copy_out`]), or, for a sample
/// no reader takes again, given back ([`PinnedPayload::release`]). Readers that hold a
/// pointer to the bytes are done before either: both run between steps.
pub struct PinnedPayload {
    state: Mutex<PinState>,
    /// The queued bindings that took the sample (examined it), each once.
    taken: AtomicU32,
}

impl PinnedPayload {
    /// The bytes of `bytes`, in place.
    #[must_use]
    pub fn new(bytes: Box<dyn PinnedBytes>) -> Arc<Self> {
        Arc::new(Self { state: Mutex::new(PinState::InPlace(bytes)), taken: AtomicU32::new(0) })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, PinState> {
        crate::locked(&self.state)
    }

    /// True while the bytes are in the transport's slot.
    #[must_use]
    pub fn in_place(&self) -> bool {
        matches!(*self.state(), PinState::InPlace(_))
    }

    /// The queued bindings that took the sample.
    #[must_use]
    pub fn taken(&self) -> u32 {
        self.taken.load(Ordering::Acquire)
    }

    /// Copy the bytes out of the slot and give it back. True when they were in place.
    pub fn copy_out(&self) -> bool {
        let mut state = self.state();
        let PinState::InPlace(held) = &*state else {
            return false;
        };
        let copy = held.bytes().to_vec();
        *state = PinState::Copied(copy);
        true
    }

    /// Give the slot back without a copy: the sample reads as no bytes from here. True when
    /// they were in place.
    pub fn release(&self) -> bool {
        let mut state = self.state();
        if !matches!(*state, PinState::InPlace(_)) {
            return false;
        }
        *state = PinState::Released;
        true
    }

    fn with<T>(&self, read: impl FnOnce(&[u8]) -> T) -> T {
        let state = self.state();
        match &*state {
            PinState::InPlace(held) => read(held.bytes()),
            PinState::Copied(copy) => read(copy),
            PinState::Released => read(&[]),
        }
    }
}

#[derive(Clone)]
enum Storage {
    Owned(Vec<u8>),
    Pinned(Arc<PinnedPayload>),
}

/// The serialized message of a history sample: a reusable buffer, or bytes read in place.
#[derive(Clone)]
pub struct SamplePayload(Storage);

impl Default for SamplePayload {
    fn default() -> Self {
        Self(Storage::Owned(Vec::new()))
    }
}

impl SamplePayload {
    /// Bytes held in place.
    #[must_use]
    pub fn pinned(payload: Arc<PinnedPayload>) -> Self {
        Self(Storage::Pinned(payload))
    }

    /// Run `read` on the bytes.
    pub fn with_bytes<T>(&self, read: impl FnOnce(&[u8]) -> T) -> T {
        match &self.0 {
            Storage::Owned(bytes) => read(bytes),
            Storage::Pinned(payload) => payload.with(read),
        }
    }

    /// The address and length of the bytes, for a C reader of the step: valid until the
    /// bytes move, which [`PinnedPayload::copy_out`] and [`PinnedPayload::release`] do
    /// between steps, and the history's reuse of an evicted buffer does at a push.
    #[must_use]
    pub fn raw_parts(&self) -> (*const u8, usize) {
        self.with_bytes(|bytes| (bytes.as_ptr(), bytes.len()))
    }

    /// The bytes, copied.
    #[must_use]
    pub fn to_vec(&self) -> Vec<u8> {
        self.with_bytes(<[u8]>::to_vec)
    }

    /// The pinned bytes, for a payload read in place.
    #[must_use]
    pub fn as_pinned(&self) -> Option<&Arc<PinnedPayload>> {
        match &self.0 {
            Storage::Owned(_) => None,
            Storage::Pinned(payload) => Some(payload),
        }
    }

    /// Count one queued binding that took the sample; nothing for an owned buffer.
    pub fn mark_taken(&self) {
        if let Storage::Pinned(payload) = &self.0 {
            payload.taken.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn capacity(&self) -> usize {
        match &self.0 {
            Storage::Owned(bytes) => bytes.capacity(),
            Storage::Pinned(_) => 0,
        }
    }
}

impl PartialEq for SamplePayload {
    fn eq(&self, other: &Self) -> bool {
        if let (Storage::Pinned(left), Storage::Pinned(right)) = (&self.0, &other.0)
            && Arc::ptr_eq(left, right)
        {
            return true;
        }
        self.with_bytes(|left| other.with_bytes(|right| left == right))
    }
}

impl Eq for SamplePayload {}

impl std::fmt::Debug for SamplePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.with_bytes(|bytes| f.debug_tuple("SamplePayload").field(&bytes.len()).finish())
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
        self.push_with(framework_sent_nanos, |reused, sequence| {
            write_sample(
                reused,
                sequence,
                payload,
                transport_sent_nanos,
                framework_sent_nanos,
                origin,
            )
        })
    }

    /// [`CycleHistory::push_from`] for bytes read in place, where they arrived.
    pub fn push_pinned(
        &mut self,
        payload: Arc<PinnedPayload>,
        transport_sent_nanos: u64,
        framework_sent_nanos: u64,
        origin: SampleOrigin,
    ) -> u64 {
        self.push_with(framework_sent_nanos, |_, sequence| {
            Arc::new(HistorySample {
                sequence,
                payload: SamplePayload::pinned(payload),
                transport_sent_nanos,
                framework_sent_nanos,
                origin,
            })
        })
    }

    /// Evict the oldest sample of a full history, then append the one `make` writes, in
    /// the evicted sample's buffer when no reader holds it.
    fn push_with(
        &mut self,
        framework_sent_nanos: u64,
        make: impl FnOnce(Option<Arc<HistorySample>>, u64) -> Arc<HistorySample>,
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
                let keep = reused
                    .as_ref()
                    .is_none_or(|current| oldest.payload.capacity() > current.payload.capacity());
                if keep {
                    reused = Some(oldest);
                }
            }
        }

        let sample = make(reused, sequence);

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
        match &mut sample.payload.0 {
            Storage::Owned(bytes) => {
                bytes.clear();
                bytes.extend_from_slice(payload);
            }
            Storage::Pinned(_) => sample.payload = SamplePayload(Storage::Owned(payload.to_vec())),
        }
        sample.transport_sent_nanos = transport_sent_nanos;
        sample.framework_sent_nanos = framework_sent_nanos;
        sample.origin = origin;
        return arc;
    }
    Arc::new(HistorySample {
        sequence,
        payload: SamplePayload(Storage::Owned(payload.to_vec())),
        transport_sent_nanos,
        framework_sent_nanos,
        origin,
    })
}
