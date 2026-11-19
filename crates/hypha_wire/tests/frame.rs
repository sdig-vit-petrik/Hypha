//! Sealing and verifying one frame header.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use hypha_wire::frame::{FLAG_LATCHED, FRAME_MAGIC, FrameMetadata, ParseError, seal, verify};

const METADATA: FrameMetadata = FrameMetadata {
    schema_hash: 0x0123_4567_89AB_CDEF,
    publisher_id: 2,
    sequence: 3,
    transport_sent_nanos: 4,
    framework_sent_nanos: 5,
};

#[test]
fn a_sealed_header_verifies_against_its_payload() {
    let header = seal(topic(), METADATA, 0, b"hello");
    assert_eq!(header.magic, FRAME_MAGIC);
    let parsed = verify(topic(), &header, b"hello").expect("a sealed frame verifies");
    assert_eq!(parsed.schema_hash(), METADATA.schema_hash);
    assert_eq!(parsed.publisher_id(), 2);
    assert_eq!(parsed.sequence(), 3);
    assert_eq!(parsed.transport_sent_nanos(), 4);
    assert_eq!(parsed.framework_sent_nanos(), 5);
    assert_eq!(parsed.payload(), b"hello");
    assert!(!parsed.latched());
}

#[test]
fn an_empty_payload_seals_and_verifies() {
    let header = seal(topic(), METADATA, 0, &[]);
    assert_eq!(verify(topic(), &header, &[]).expect("verifies").payload(), [] as [u8; 0]);
}

#[test]
fn the_latched_flag_is_carried_and_reported() {
    let header = seal(topic(), METADATA, FLAG_LATCHED, b"x");
    assert!(verify(topic(), &header, b"x").expect("verifies").latched());
}

#[test]
fn another_magic_is_refused_before_the_integrity_code() {
    let mut header = seal(topic(), METADATA, 0, b"x");
    header.magic ^= 1;
    assert_eq!(verify(topic(), &header, b"x"), Err(ParseError::UnsupportedMagic));
}

#[test]
fn a_changed_header_field_is_refused() {
    let sealed = seal(topic(), METADATA, 0, b"hello");
    let mut changed = [sealed; 7];
    changed[0].flags ^= FLAG_LATCHED;
    changed[1].metadata.schema_hash ^= 1;
    changed[2].metadata.publisher_id ^= 1;
    changed[3].metadata.sequence ^= 1;
    changed[4].metadata.transport_sent_nanos ^= 1;
    changed[5].metadata.framework_sent_nanos ^= 1;
    changed[6].crc ^= 1;
    for header in changed {
        assert_eq!(verify(topic(), &header, b"hello"), Err(ParseError::CrcMismatch), "{header:?}");
    }
}

#[test]
fn the_extreme_stamps_round_trip() {
    let metadata = FrameMetadata {
        schema_hash: u64::MAX,
        publisher_id: u64::MAX,
        sequence: u64::MAX,
        transport_sent_nanos: u64::MAX,
        framework_sent_nanos: 0,
    };
    let header = seal(topic(), metadata, u32::MAX, b"");
    let parsed = verify(topic(), &header, b"").expect("verifies");
    assert_eq!(parsed.framework_sent_nanos(), 0);
    assert_eq!(parsed.transport_sent_nanos(), u64::MAX);
}

#[test]
fn a_frame_sealed_for_one_topic_fails_its_crc_on_another() {
    let pose = hypha_wire::frame::TopicCode::of("/robot/pose");
    let command = hypha_wire::frame::TopicCode::of("/robot/command");
    let header = seal(pose, METADATA, 0, b"hello");
    assert!(verify(pose, &header, b"hello").is_ok());
    assert_eq!(verify(command, &header, b"hello").unwrap_err(), ParseError::CrcMismatch);
}

#[test]
fn the_topic_code_follows_every_byte_of_the_name() {
    use hypha_wire::frame::TopicCode;
    let code = TopicCode::of("/a/b");
    assert_eq!(code, TopicCode::of("/a/b"));
    for other in ["/a/c", "/a/b/", "a/b", "/A/b", ""] {
        assert_ne!(code, TopicCode::of(other), "{other}");
    }
}

#[test]
fn a_frame_of_the_previous_format_is_refused_by_its_magic() {
    let mut header = seal(topic(), METADATA, 0, b"hello");
    header.magic = 0x3658_4F49;
    assert_eq!(verify(topic(), &header, b"hello").unwrap_err(), ParseError::UnsupportedMagic);
}

/// The topic every frame of this file is sealed for.
fn topic() -> hypha_wire::frame::TopicCode {
    hypha_wire::frame::TopicCode::of("/test")
}
