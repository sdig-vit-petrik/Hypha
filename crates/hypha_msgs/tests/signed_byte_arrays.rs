//! Signed byte arrays, `byte[]` and `int8[]`, round-trip through the generated codec.

use hypha_core::message::{DecodeOwned, EncodeInto};
use hypha_msgs::std_msgs::{ByteMultiArray, Int8MultiArray};

/// The wire bytes of a message, sized by its own `encoded_len`.
#[allow(clippy::expect_used)]
fn encoded<T: EncodeInto>(message: &T) -> Vec<u8> {
    let mut buffer = vec![0_u8; message.encoded_len()];
    let written = message.encode_into(&mut buffer).expect("the buffer is sized to the message");
    assert_eq!(written, buffer.len());
    buffer
}

/// Points the four byte count that precedes `count_bytes` data bytes at `count`.
fn set_count(wire: &mut [u8], data_bytes: usize, count: u32) {
    let count_at = wire.len().saturating_sub(data_bytes.saturating_add(4));
    if let Some(slot) = wire.get_mut(count_at..count_at.saturating_add(4)) {
        slot.copy_from_slice(&count.to_le_bytes());
    }
}

#[test]
#[allow(clippy::expect_used)]
fn a_byte_array_round_trips_with_each_element_as_its_signed_byte() {
    let original = ByteMultiArray { data: vec![-1, 0, 127, -128], ..Default::default() };
    let wire = encoded(&original);

    // The count, then the four data bytes as two's complement, close the message.
    assert_eq!(
        wire.get(wire.len().saturating_sub(8)..),
        Some(&[4, 0, 0, 0, 0xFF, 0x00, 0x7F, 0x80][..])
    );
    let decoded = ByteMultiArray::decode_from(&wire).expect("the round trip decodes");
    assert_eq!(decoded, original);
}

#[test]
#[allow(clippy::expect_used)]
fn an_int8_array_round_trips_with_each_element_as_its_signed_byte() {
    let original = Int8MultiArray { data: vec![-128, 1, -2], ..Default::default() };
    let wire = encoded(&original);

    assert_eq!(wire.get(wire.len().saturating_sub(7)..), Some(&[3, 0, 0, 0, 0x80, 0x01, 0xFE][..]));
    let decoded = Int8MultiArray::decode_from(&wire).expect("the round trip decodes");
    assert_eq!(decoded, original);
}

#[test]
fn a_byte_array_count_past_the_bytes_present_is_refused() {
    let original = ByteMultiArray { data: vec![1, 2], ..Default::default() };
    let mut wire = encoded(&original);
    set_count(&mut wire, 2, 5);

    assert!(
        ByteMultiArray::decode_from(&wire).is_err(),
        "a count of five was accepted for two bytes"
    );
}

#[test]
fn an_int8_array_count_past_the_bytes_present_is_refused() {
    let original = Int8MultiArray { data: vec![1, 2], ..Default::default() };
    let mut wire = encoded(&original);
    set_count(&mut wire, 2, 5);

    assert!(
        Int8MultiArray::decode_from(&wire).is_err(),
        "a count of five was accepted for two bytes"
    );
}
