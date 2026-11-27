//! Sequence validation for each publisher, and publisher identity allocation.
use alloc::collections::BTreeMap;
use core::num::NonZeroUsize;
use core::sync::atomic::{AtomicU64, Ordering};

/// Half the sequence range: the largest forward distance that still counts as
/// newer, plus one.
const NEWER_WINDOW: u64 = 1 << 63;

/// The most publishers one validator tracks.
///
/// Bounds a subscription whose publishers restart for years.
pub const MAX_TRACKED_PUBLISHERS: usize = 4096;

/// The last accepted sequence of one publisher, and when it was accepted.
#[derive(Debug, Clone, Copy)]
struct Tracked {
    sequence: u64,
    /// The validator's call ordinal at the last accepted frame.
    accepted_at: u64,
}

/// Tracks the last accepted sequence for each publisher.
///
/// When the table is full, a new publisher makes the validator forget the publisher
/// that sent least recently; a forgotten publisher's next frame is accepted as its first.
#[derive(Debug)]
pub struct SequenceValidator {
    publishers: BTreeMap<u64, Tracked>,
    capacity: NonZeroUsize,
    /// Counts calls to [`SequenceValidator::accept`], to order publishers by
    calls: u64,
    /// Missing sequence values between accepted forward frames.
    missing_sequences: u64,
    /// Frames the publishers sent before the first one accepted from each: a publisher's first
    /// frame has sequence 1, so the first sequence accepted, less one, is the count before it.
    sent_before_first: u64,
}

impl Default for SequenceValidator {
    fn default() -> Self {
        let capacity = NonZeroUsize::new(MAX_TRACKED_PUBLISHERS).unwrap_or(NonZeroUsize::MIN);
        Self::with_capacity(capacity)
    }
}

impl SequenceValidator {
    /// Create an empty validator that tracks [`MAX_TRACKED_PUBLISHERS`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty validator that tracks at most `capacity` publishers.
    #[must_use]
    pub const fn with_capacity(capacity: NonZeroUsize) -> Self {
        Self {
            publishers: BTreeMap::new(),
            capacity,
            calls: 0,
            missing_sequences: 0,
            sent_before_first: 0,
        }
    }

    /// Accept the first sequence of a publisher, or one after the last one
    /// accepted.
    ///
    /// Measured modulo the sequence range: a sequence less than half the range ahead of
    /// the last accepted one is newer, gaps included. Anything else is refused.
    pub fn accept(&mut self, publisher_id: u64, sequence: u64) -> bool {
        self.accept_as(publisher_id, sequence, true)
    }

    /// As [`SequenceValidator::accept`], saying whether the first sequence of a publisher
    /// counts as frames sent before the first one accepted. It does not for a frame that a
    /// latched publisher hands to a late subscriber: the last sample is all that is wanted.
    pub fn accept_as(&mut self, publisher_id: u64, sequence: u64, counts_first: bool) -> bool {
        self.calls = self.calls.wrapping_add(1);
        let now = self.calls;
        let Some(tracked) = self.publishers.get_mut(&publisher_id) else {
            if self.publishers.len() >= self.capacity.get() {
                self.forget_least_recent();
            }
            self.publishers.insert(publisher_id, Tracked { sequence, accepted_at: now });
            if counts_first {
                // A publisher's first frame has sequence 1.
                self.sent_before_first =
                    self.sent_before_first.saturating_add(sequence.saturating_sub(1));
            }
            return true;
        };
        let distance = sequence.wrapping_sub(tracked.sequence);
        let is_newer = distance != 0 && distance < NEWER_WINDOW;
        if !is_newer {
            return false;
        }
        self.missing_sequences = self.missing_sequences.wrapping_add(distance.saturating_sub(1));
        tracked.sequence = sequence;
        tracked.accepted_at = now;
        true
    }

    /// Number of missing sequence values between accepted forward frames.
    #[must_use]
    pub const fn missing_sequences(&self) -> u64 {
        self.missing_sequences
    }

    /// Frames the publishers sent before the first one accepted from each, a latched replay
    /// not counted. A publisher that existed before the subscription is among them: whether
    /// they are a loss is for the reader to say.
    #[must_use]
    pub const fn sent_before_first(&self) -> u64 {
        self.sent_before_first
    }

    /// Forget the publisher whose last accepted frame is the oldest.
    /// Runs only when a new publisher arrives at a full table.
    fn forget_least_recent(&mut self) {
        let now = self.calls;
        let least_recent = self
            .publishers
            .iter()
            .max_by_key(|(_, tracked)| now.wrapping_sub(tracked.accepted_at))
            .map(|(publisher_id, _)| *publisher_id);
        if let Some(publisher_id) = least_recent {
            self.publishers.remove(&publisher_id);
        }
    }
}
/// The sequence number a sender puts on the frame after `previous`.
/// It wraps to zero after the maximum value.
#[must_use]
pub const fn next_sequence(previous: u64) -> u64 {
    previous.wrapping_add(1)
}

/// Allocate a process-unique publisher identifier.
/// Identifiers repeat only after 2^64 publishers in one process.
pub fn next_publisher_id(monotonic_nanos: u64, process_id: u32) -> u64 {
    static SEED: AtomicU64 = AtomicU64::new(0);
    static ORDINAL: AtomicU64 = AtomicU64::new(0);
    let candidate = (monotonic_nanos.rotate_left(17) ^ u64::from(process_id).rotate_left(41)) | 1;
    let _ = SEED.compare_exchange(0, candidate, Ordering::Relaxed, Ordering::Relaxed);
    let seed = SEED.load(Ordering::Relaxed);
    let ordinal = ORDINAL.fetch_add(1, Ordering::Relaxed);
    publisher_id_from(seed, ordinal)
}

/// The publisher identifier for the `ordinal`-th publisher of a process.
/// Multiplication by an odd constant is a bijection modulo 2^64.
#[must_use]
pub const fn publisher_id_from(seed: u64, ordinal: u64) -> u64 {
    seed.wrapping_add(ordinal).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}
