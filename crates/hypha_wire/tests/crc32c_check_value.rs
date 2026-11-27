//! The safety code is the standard CRC-32C (Castagnoli), pinned three ways: to the
//! published check values, to a frame sealed by the earlier implementation (the `crc32c`
//! crate, 0.6.8), and to that implementation over arbitrary lengths and alignments. A
//! frame of an older build verifies in a newer one and the other way round.
#![expect(clippy::expect_used, reason = "a test stops at a slice outside its own pattern")]

use crc_fast::crc32_iscsi;
use hypha_wire::frame::{FLAG_LATCHED, FrameHeader, FrameMetadata, TopicCode, seal, verify};
use proptest::collection::vec;
use proptest::prelude::*;

/// The check value CRC-32C publishes for the input `123456789`.
const PUBLISHED_CHECK_VALUE: u32 = 0xE306_9283;

const METADATA: FrameMetadata = FrameMetadata {
    schema_hash: 0x0123_4567_89AB_CDEF,
    publisher_id: 2,
    sequence: 3,
    transport_sent_nanos: 4,
    framework_sent_nanos: 5,
};

/// The integrity code as hypha computed it with `crc32c` 0.6.8: the topic code, then the
/// little-endian magic, flags and metadata, then the payload.
fn earlier_integrity_code(topic: &str, header: &FrameHeader, payload: &[u8]) -> u32 {
    let m = header.metadata;
    let mut crc =
        crc32c::crc32c_append(crc32c::crc32c(topic.as_bytes()), &header.magic.to_le_bytes());
    crc = crc32c::crc32c_append(crc, &header.flags.to_le_bytes());
    for field in
        [m.schema_hash, m.publisher_id, m.sequence, m.transport_sent_nanos, m.framework_sent_nanos]
    {
        crc = crc32c::crc32c_append(crc, &field.to_le_bytes());
    }
    crc32c::crc32c_append(crc, payload)
}

/// `len` bytes that are not a repeating pattern of a short period.
fn pattern(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_u32;
    (0..len)
        .map(|_| {
            state ^= state.wrapping_shl(13);
            state ^= state.wrapping_shr(17);
            state ^= state.wrapping_shl(5);
            state.to_le_bytes()[0]
        })
        .collect()
}

#[test]
fn crc32c_matches_the_published_check_values() {
    assert_eq!(
        crc32_iscsi(b"123456789"),
        PUBLISHED_CHECK_VALUE,
        "the safety code must be the standard CRC-32C (Castagnoli)"
    );
    // RFC 3720, B.4: 32 bytes of zeros, of ones, ascending and descending.
    let ascending: Vec<u8> = (0..32).collect();
    let descending: Vec<u8> = (0..32).rev().collect();
    assert_eq!(crc32_iscsi(&[0; 32]), 0x8A91_36AA);
    assert_eq!(crc32_iscsi(&[0xFF; 32]), 0x62A8_AB43);
    assert_eq!(crc32_iscsi(&ascending), 0x46DD_794E);
    assert_eq!(crc32_iscsi(&descending), 0x113F_DB5C);
}

#[test]
fn crc32c_of_no_bytes_is_zero() {
    assert_eq!(crc32_iscsi(b""), 0);
}

#[test]
fn crc32c_detects_a_single_flipped_bit() {
    let clean = crc32_iscsi(b"safety payload");
    let flipped = crc32_iscsi(b"safety payloae");
    assert_ne!(clean, flipped);
}

#[test]
fn crc32c_depends_on_byte_order() {
    assert_ne!(crc32_iscsi(b"ab"), crc32_iscsi(b"ba"));
}

#[test]
fn a_frame_is_sealed_with_the_code_the_earlier_implementation_gave_it() {
    let topic = "/camera/image_raw";
    let payload = pattern(1024 * 1024 + 3);
    let header = seal(TopicCode::of(topic), METADATA, FLAG_LATCHED, &payload);
    assert_eq!(earlier_integrity_code(topic, &header, &payload), SEALED_BY_0_6_8);
    assert_eq!(header.crc, SEALED_BY_0_6_8);
    assert!(verify(TopicCode::of(topic), &header, &payload).is_ok());
}

/// The integrity code of the frame above, sealed by hypha with `crc32c` 0.6.8.
const SEALED_BY_0_6_8: u32 = 0x144D_FD1A;

#[test]
fn the_integrity_code_equals_the_earlier_one_at_every_size_and_alignment() {
    let topic = "/points";
    let bytes = pattern(3 * 8192 * 3 + 64);
    // Every length up to 1 KiB and every start in an 8-byte word, then lengths around the
    // block sizes of both implementations (128 B, 1 KiB, 3 x 256 B, 3 x 8 KiB).
    let lengths = (0..=1024).chain([3 * 256 - 1, 3 * 256, 3 * 256 + 1, 4095, 4096, 4097]).chain([
        3 * 8192 - 1,
        3 * 8192,
        3 * 8192 + 1,
        2 * 3 * 8192 + 7,
        3 * 3 * 8192 + 9,
    ]);
    for len in lengths {
        for start in 0..8 {
            let payload =
                bytes.get(start..).and_then(|tail| tail.get(..len)).expect("in the pattern");
            let header = seal(TopicCode::of(topic), METADATA, 0, payload);
            assert_eq!(
                header.crc,
                earlier_integrity_code(topic, &header, payload),
                "{len} at {start}"
            );
        }
    }
}

proptest! {
    #[test]
    fn the_integrity_code_equals_the_earlier_one(
        topic in "/[a-z_/]{0,40}",
        flags in any::<u32>(),
        sequence in any::<u64>(),
        start in 0_usize..64,
        payload in vec(any::<u8>(), 0..70_000),
    ) {
        let payload = payload.get(start.min(payload.len())..).unwrap_or_default();
        let metadata = FrameMetadata { sequence, ..METADATA };
        let header = seal(TopicCode::of(&topic), metadata, flags, payload);
        prop_assert_eq!(header.crc, earlier_integrity_code(&topic, &header, payload));
        prop_assert!(verify(TopicCode::of(&topic), &header, payload).is_ok());
    }
}
