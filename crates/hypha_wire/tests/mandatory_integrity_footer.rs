//! The integrity code covers the whole header and the payload, and is mandatory.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use hypha_wire::frame::{FrameHeader, FrameMetadata, ParseError, seal, verify};

fn sealed(payload: &[u8]) -> FrameHeader {
    seal(
        topic(),
        FrameMetadata {
            schema_hash: 1,
            publisher_id: 2,
            sequence: 3,
            transport_sent_nanos: 4,
            framework_sent_nanos: 5,
        },
        0,
        payload,
    )
}

#[test]
fn every_payload_byte_is_covered() {
    let payload = b"payload";
    let header = sealed(payload);
    for index in 0..payload.len() {
        let mut changed = payload.to_vec();
        changed[index] ^= 0x80;
        assert_eq!(
            verify(topic(), &header, &changed),
            Err(ParseError::CrcMismatch),
            "byte {index}"
        );
    }
    assert!(verify(topic(), &header, payload).is_ok());
}

#[test]
fn an_unsealed_zero_crc_is_refused() {
    let mut header = sealed(b"x");
    header.crc = 0;
    assert_eq!(verify(topic(), &header, b"x"), Err(ParseError::CrcMismatch));
}

/// The topic every frame of this file is sealed for.
fn topic() -> hypha_wire::frame::TopicCode {
    hypha_wire::frame::TopicCode::of("/test")
}
