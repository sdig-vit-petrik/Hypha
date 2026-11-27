//! Transport-independent frame acceptance state.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use hypha_wire::frame::{FrameHeader, ParseError, ParsedFrame, TopicCode, verify};
use hypha_wire::integrity::SequenceValidator;

/// Why a subscriber refused one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRejection {
    /// The frame layout or validator state is invalid.
    InvalidFrame,
    /// The frame checksum does not match its bytes.
    CorruptPayload,
    /// The frame names another schema than the stream it arrived on.
    SchemaMismatch,
    /// The send time is later than the current host time.
    Future,
    /// The sender sequence was not newer than the last one accepted.
    Sequence,
}

impl core::fmt::Display for FrameRejection {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        out.write_str(match self {
            Self::InvalidFrame => "invalid frame",
            Self::CorruptPayload => "CRC mismatch",
            Self::SchemaMismatch => "schema mismatch",
            Self::Future => "send time in the future",
            Self::Sequence => "sequence not newer than the last accepted (duplicate or reordered)",
        })
    }
}

/// One snapshot of frame rejection counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameValidationStats {
    /// Invalid frame layouts or validator state failures.
    pub invalid_frame: u64,
    /// Checksum failures.
    pub corrupt_payload: u64,
    /// Schema expectation failures.
    pub schema_mismatch: u64,
    /// Duplicate or reordered sender sequences.
    pub sequence_gap: u64,
    /// Future send timestamps.
    pub invalid_timestamp: u64,
}

#[derive(Debug, Default)]
struct RejectionCounters {
    invalid_frame: AtomicU64,
    corrupt_payload: AtomicU64,
    schema_mismatch: AtomicU64,
    sequence_gap: AtomicU64,
    invalid_timestamp: AtomicU64,
}

/// Validates frames for one subscription.
#[derive(Debug)]
pub struct FrameValidator {
    expected_schema_hash: Option<u64>,
    sequences: Mutex<SequenceValidator>,
    missing_sequences: AtomicU64,
    sent_before_first: AtomicU64,
    counters: RejectionCounters,
}

impl FrameValidator {
    /// Create a validator for one subscription contract.
    #[must_use]
    pub fn new(expected_schema_hash: Option<u64>) -> Self {
        Self {
            expected_schema_hash,
            sequences: Mutex::new(SequenceValidator::default()),
            missing_sequences: AtomicU64::new(0),
            sent_before_first: AtomicU64::new(0),
            counters: RejectionCounters::default(),
        }
    }

    /// Validate one sample that arrived on `topic`, its header and payload, at
    /// `now_nanos`. `Ok` carries the transport send time of the accepted frame.
    ///
    /// # Errors
    /// Returns the first rule that refuses the frame.
    pub fn validate(
        &self,
        topic: TopicCode,
        header: &FrameHeader,
        payload: &[u8],
        now_nanos: u64,
    ) -> Result<u64, FrameRejection> {
        let mut cached_now = now_nanos;
        self.validate_with_clock(topic, header, payload, &mut cached_now, || now_nanos)
            .map(|frame| frame.transport_sent_nanos())
    }

    /// Validate once and return the borrowed frame, reusing a batch's host time.
    /// Refresh the clock only if a valid, schema-matching frame is newer than the cached
    /// time. A real future stamp is still refused and counted.
    ///
    /// # Errors
    /// Returns the first rule that refuses the frame, without updating sequence
    /// state for a refused timestamp.
    pub fn validate_with_clock<'a>(
        &self,
        topic: TopicCode,
        header: &FrameHeader,
        payload: &'a [u8],
        now_nanos: &mut u64,
        refresh: impl FnOnce() -> u64,
    ) -> Result<ParsedFrame<'a>, FrameRejection> {
        let frame = match verify(topic, header, payload) {
            Ok(frame) => frame,
            // A payload_len past the slice cannot be checked: a corrupt frame, as a bad CRC.
            Err(ParseError::CrcMismatch | ParseError::LengthMismatch) => {
                self.counters.corrupt_payload.fetch_add(1, Ordering::Relaxed);
                return Err(FrameRejection::CorruptPayload);
            }
            Err(_) => {
                self.counters.invalid_frame.fetch_add(1, Ordering::Relaxed);
                return Err(FrameRejection::InvalidFrame);
            }
        };

        let schema_mismatch =
            self.expected_schema_hash.is_some_and(|expected| frame.schema_hash() != expected);
        if schema_mismatch {
            self.counters.schema_mismatch.fetch_add(1, Ordering::Relaxed);
            return Err(FrameRejection::SchemaMismatch);
        }

        if frame.transport_sent_nanos() > *now_nanos {
            *now_nanos = refresh();
        }
        if frame.transport_sent_nanos() > *now_nanos {
            self.counters.invalid_timestamp.fetch_add(1, Ordering::Relaxed);
            return Err(FrameRejection::Future);
        }

        let mut sequences = crate::locked(&self.sequences);
        if sequences.accept_as(frame.publisher_id(), frame.sequence(), !frame.latched()) {
            self.missing_sequences.store(sequences.missing_sequences(), Ordering::Release);
            self.sent_before_first.store(sequences.sent_before_first(), Ordering::Release);
            return Ok(frame);
        }
        self.counters.sequence_gap.fetch_add(1, Ordering::Relaxed);
        Err(FrameRejection::Sequence)
    }

    /// Read all rejection counters at one instant.
    #[must_use]
    pub fn stats(&self) -> FrameValidationStats {
        FrameValidationStats {
            invalid_frame: self.counters.invalid_frame.load(Ordering::Acquire),
            corrupt_payload: self.counters.corrupt_payload.load(Ordering::Acquire),
            schema_mismatch: self.counters.schema_mismatch.load(Ordering::Acquire),
            sequence_gap: self.counters.sequence_gap.load(Ordering::Acquire),
            invalid_timestamp: self.counters.invalid_timestamp.load(Ordering::Acquire),
        }
    }

    /// Frames the publishers of this subscription sent before the first one accepted from
    /// each (see [`SequenceValidator::sent_before_first`]).
    #[must_use]
    pub fn sent_before_first(&self) -> u64 {
        self.sent_before_first.load(Ordering::Acquire)
    }

    /// Total missing sequence values across accepted forward frames.
    #[must_use]
    pub fn missing_sequences(&self) -> u64 {
        self.missing_sequences.load(Ordering::Acquire)
    }
}
