//! Frame validation behavior.

use hypha_core::frame_validation::{FrameRejection, FrameValidator};
use hypha_wire::frame::{FrameHeader, FrameMetadata, seal};

const PAYLOAD: &[u8] = b"x";

fn frame(schema_hash: u64, sequence: u64, sent: u64) -> FrameHeader {
    let metadata = FrameMetadata {
        schema_hash,
        publisher_id: 7,
        sequence,
        transport_sent_nanos: sent,
        framework_sent_nanos: sent,
    };
    seal(metadata, 0, PAYLOAD)
}

#[test]
fn accepts_matching_schema_and_increasing_sequence_and_returns_the_send_time() {
    let validator = FrameValidator::new(Some(11));
    assert_eq!(validator.validate(&frame(11, 1, 900), PAYLOAD, 1_000), Ok(900));
    assert_eq!(validator.validate(&frame(11, 2, 950), PAYLOAD, 1_000), Ok(950));
    assert_eq!(validator.stats().schema_mismatch, 0);
}

#[test]
fn rejects_schema_mismatch_and_counts_it() {
    let validator = FrameValidator::new(Some(11));
    assert_eq!(
        validator.validate(&frame(12, 1, 900), PAYLOAD, 1_000),
        Err(FrameRejection::SchemaMismatch)
    );
    assert_eq!(validator.stats().schema_mismatch, 1);
}

#[test]
fn zero_expectation_accepts_any_schema() {
    let validator = FrameValidator::new(None);
    assert_eq!(validator.validate(&frame(12, 1, 900), PAYLOAD, 1_000), Ok(900));
}

#[test]
fn rejects_future_and_reordered_but_does_not_reject_old_transport_age() {
    let validator = FrameValidator::new(Some(11));
    assert_eq!(
        validator.validate(&frame(11, 1, 1_001), PAYLOAD, 1_000),
        Err(FrameRejection::Future)
    );
    assert_eq!(validator.validate(&frame(11, 1, 1), PAYLOAD, 1_000), Ok(1));
    assert_eq!(validator.validate(&frame(11, 2, 950), PAYLOAD, 1_000), Ok(950));
    assert_eq!(
        validator.validate(&frame(11, 2, 951), PAYLOAD, 1_000),
        Err(FrameRejection::Sequence)
    );
    let stats = validator.stats();
    assert_eq!(stats.invalid_timestamp, 1);
    assert_eq!(stats.sequence_gap, 1);
}

#[test]
fn rejects_a_foreign_magic_as_an_invalid_frame() {
    let validator = FrameValidator::new(None);
    let mut foreign = frame(11, 1, 900);
    foreign.magic ^= 1;
    assert_eq!(validator.validate(&foreign, PAYLOAD, 1_000), Err(FrameRejection::InvalidFrame));
    assert_eq!(validator.stats().invalid_frame, 1);
}

#[test]
fn a_corrupt_payload_byte_is_refused_as_corrupt() {
    let validator = FrameValidator::new(None);
    assert_eq!(
        validator.validate(&frame(11, 1, 900), b"y", 1_000),
        Err(FrameRejection::CorruptPayload)
    );
    assert_eq!(validator.stats().corrupt_payload, 1);
}

#[test]
fn a_corrupt_header_field_is_refused_as_corrupt() {
    let validator = FrameValidator::new(None);
    let mut corrupt = frame(11, 1, 900);
    corrupt.metadata.sequence ^= 1;
    assert_eq!(validator.validate(&corrupt, PAYLOAD, 1_000), Err(FrameRejection::CorruptPayload));
    let mut flipped = frame(11, 1, 900);
    flipped.flags ^= 1;
    assert_eq!(validator.validate(&flipped, PAYLOAD, 1_000), Err(FrameRejection::CorruptPayload));
    let stats = validator.stats();
    assert_eq!(stats.invalid_frame, 0);
    assert_eq!(stats.corrupt_payload, 2);
}

#[test]
fn a_sequence_that_wraps_past_its_maximum_is_accepted_as_the_next_one() {
    let validator = FrameValidator::new(Some(11));
    assert_eq!(validator.validate(&frame(11, u64::MAX, 900), PAYLOAD, 1_000), Ok(900));
    assert_eq!(validator.validate(&frame(11, 0, 950), PAYLOAD, 1_000), Ok(950));
    assert_eq!(validator.stats().sequence_gap, 0);
}

#[test]
#[allow(clippy::expect_used)]
fn a_batch_reuses_its_clock_and_refreshes_only_for_newer_send_times() {
    let validator = FrameValidator::new(Some(11));
    let mut now = 1_000;
    let header = frame(11, 1, 900);
    let unexpected_refreshes = std::cell::Cell::new(0_u32);
    let cached_only = || {
        unexpected_refreshes.set(unexpected_refreshes.get().saturating_add(1));
        0
    };
    let accepted = validator
        .validate_with_clock(&header, PAYLOAD, &mut now, cached_only)
        .expect("valid frame");
    assert_eq!(accepted.payload(), PAYLOAD);
    assert_eq!(accepted.framework_sent_nanos(), 900);
    assert!(
        validator.validate_with_clock(&frame(11, 2, 1_050), PAYLOAD, &mut now, || 1_100).is_ok()
    );
    assert_eq!(now, 1_100);
    assert_eq!(
        validator.validate_with_clock(&frame(11, 3, 1_200), PAYLOAD, &mut now, || 1_150),
        Err(FrameRejection::Future)
    );
    assert_eq!(validator.stats().invalid_timestamp, 1);
    // A refused future timestamp never advances the sequence validator.
    assert!(
        validator.validate_with_clock(&frame(11, 3, 1_140), PAYLOAD, &mut now, cached_only).is_ok()
    );
    assert_eq!(unexpected_refreshes.get(), 0);
    assert_eq!(validator.missing_sequences(), 0);
}

#[test]
fn the_missing_sequence_snapshot_tracks_accepted_gaps_and_wraps() {
    let validator = FrameValidator::new(None);
    assert_eq!(validator.missing_sequences(), 0);
    for (sequence, missing) in [(u64::MAX - 2, 0), (1, 3), (5, 6)] {
        assert!(validator.validate(&frame(11, sequence, 0), PAYLOAD, 0).is_ok());
        assert_eq!(validator.missing_sequences(), missing);
    }
    assert_eq!(validator.validate(&frame(11, 5, 0), PAYLOAD, 0), Err(FrameRejection::Sequence));
    assert_eq!(validator.missing_sequences(), 6);
}
