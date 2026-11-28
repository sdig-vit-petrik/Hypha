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
fn a_frame_of_0_1_0_is_refused_by_its_crc() {
    // 0.1.0 sealed the same layout with a CRC that starts from 0, not the topic code.
    let mut header = seal(topic(), METADATA, 0, b"hello");
    let m = METADATA;
    let mut crc = crc32c::crc32c(&header.magic.to_le_bytes());
    crc = crc32c::crc32c_append(crc, &header.flags.to_le_bytes());
    for field in
        [m.schema_hash, m.publisher_id, m.sequence, m.transport_sent_nanos, m.framework_sent_nanos]
    {
        crc = crc32c::crc32c_append(crc, &field.to_le_bytes());
    }
    header.crc = crc32c::crc32c_append(crc, b"hello");
    assert_eq!(verify(topic(), &header, b"hello").unwrap_err(), ParseError::CrcMismatch);
}

/// The topic every frame of this file is sealed for.
fn topic() -> hypha_wire::frame::TopicCode {
    hypha_wire::frame::TopicCode::of("/test")
}

/// The `wire_frame` fuzz seeds are `IOX7` frames sealed for `/test`: they reach the accepting
/// path of the target, and a layout change that forgets them fails here.
#[test]
#[allow(clippy::expect_used)]
fn the_fuzz_seed_frames_verify() {
    let seeds =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/wire_frame");
    for (name, payload_len) in [("seed-sealed", 5_usize), ("seed-latched-empty", 0)] {
        let bytes = std::fs::read(seeds.join(name)).expect("seed");
        let (head, slice) = bytes.split_at(56);
        let word = |at: usize| {
            u32::from_le_bytes(head.get(at..at + 4).expect("u32").try_into().expect("4 bytes"))
        };
        let long = |at: usize| {
            u64::from_le_bytes(head.get(at..at + 8).expect("u64").try_into().expect("8 bytes"))
        };
        let header = hypha_wire::frame::FrameHeader {
            magic: word(0),
            flags: word(4),
            metadata: FrameMetadata {
                schema_hash: long(8),
                publisher_id: long(16),
                sequence: long(24),
                transport_sent_nanos: long(32),
                framework_sent_nanos: long(40),
            },
            crc: word(48),
            payload_len: word(52),
        };
        assert_eq!(header.magic, FRAME_MAGIC, "{name}");
        let parsed = verify(hypha_wire::frame::TopicCode::of("/test"), &header, slice)
            .expect("the seed verifies");
        assert_eq!(parsed.payload().len(), payload_len, "{name}");
    }
}
