//! A sealed frame carrying codec-written payload round-trips through the codec.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use hypha_wire::codec::Reader;
use hypha_wire::frame::{FrameMetadata, ParseError, seal, verify};
use hypha_wire::slice_writer::SliceWriter;

fn payload() -> Vec<u8> {
    let mut buffer = [0_u8; 16];
    let len = {
        let mut writer = SliceWriter::new(&mut buffer);
        writer.write_u32(7).expect("sequence fits");
        writer.write_str("hello").expect("string fits");
        writer.written()
    };
    buffer[..len].to_vec()
}

fn metadata() -> FrameMetadata {
    FrameMetadata {
        schema_hash: 1,
        publisher_id: 2,
        sequence: 3,
        transport_sent_nanos: 4,
        framework_sent_nanos: 5,
    }
}

#[test]
fn a_framed_message_survives_codec_round_trip() {
    let payload = payload();
    let header = seal(topic(), metadata(), 0, &payload);
    let parsed = verify(topic(), &header, &payload).expect("frame verifies");
    let mut reader = Reader::new(parsed.payload());
    assert_eq!(reader.read_u32(), Ok(7));
    assert_eq!(reader.read_string(), Ok(&b"hello"[..]));
    assert!(reader.is_empty());
}

#[test]
fn a_truncated_payload_is_refused() {
    let payload = payload();
    let header = seal(topic(), metadata(), 0, &payload);
    assert_eq!(
        verify(topic(), &header, &payload[..payload.len() - 1]),
        Err(ParseError::LengthMismatch),
        "the header's payload_len is one byte past the slice"
    );
}

/// The topic every frame of this file is sealed for.
fn topic() -> hypha_wire::frame::TopicCode {
    hypha_wire::frame::TopicCode::of("/test")
}
