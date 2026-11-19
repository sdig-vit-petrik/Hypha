//! The safety code is the standard CRC-32C: pinned to the published check value.

use crc32c::crc32c;

/// The check value CRC-32C publishes for the input `123456789`.
const PUBLISHED_CHECK_VALUE: u32 = 0xE306_9283;

#[test]
fn crc32c_matches_the_published_check_value() {
    assert_eq!(
        crc32c(b"123456789"),
        PUBLISHED_CHECK_VALUE,
        "the safety code must be the standard CRC-32C (Castagnoli)"
    );
}

#[test]
fn crc32c_of_no_bytes_is_zero() {
    assert_eq!(crc32c(b""), 0);
}

#[test]
fn crc32c_detects_a_single_flipped_bit() {
    let clean = crc32c(b"safety payload");
    let flipped = crc32c(b"safety payloae");
    assert_ne!(clean, flipped);
}

#[test]
fn crc32c_depends_on_byte_order() {
    assert_ne!(crc32c(b"ab"), crc32c(b"ba"));
}
