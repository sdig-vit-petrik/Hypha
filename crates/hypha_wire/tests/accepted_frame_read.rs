//! `ParsedFrame::accepted` exposes an already-verified header without checking it again.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use hypha_wire::frame::{FrameMetadata, ParseError, ParsedFrame, seal, verify};

fn metadata() -> FrameMetadata {
    FrameMetadata {
        schema_hash: 9,
        publisher_id: 8,
        sequence: 7,
        transport_sent_nanos: 6,
        framework_sent_nanos: 5,
    }
}

#[test]
fn an_accepted_frame_reads_as_verify_reads_it() {
    let header = seal(metadata(), 0, b"abc");
    assert_eq!(ParsedFrame::accepted(&header, b"abc"), verify(&header, b"abc").expect("verifies"));
}

#[test]
fn accepted_does_not_compute_the_integrity_code() {
    let mut header = seal(metadata(), 0, b"abc");
    header.crc ^= 1;
    assert_eq!(verify(&header, b"abc"), Err(ParseError::CrcMismatch));
    let read = ParsedFrame::accepted(&header, b"abc");
    assert_eq!(read.sequence(), 7);
    assert_eq!(read.payload(), b"abc");
}
