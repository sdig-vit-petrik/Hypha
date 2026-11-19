//! An untrusted element count is checked before it reserves memory.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use hypha_wire::codec::{CodecError, MAX_UNBOUNDED_ARRAY_ELEMENTS, Reader};

fn count_only(count: u32) -> Vec<u8> {
    count.to_le_bytes().to_vec()
}

#[test]
fn a_fixed_width_count_larger_than_the_buffer_is_refused() {
    let bytes = count_only(1_000_000);
    let mut reader = Reader::new(&bytes);

    assert_eq!(
        reader.read_array_len_bounded(8),
        Err(CodecError::LengthOutOfRange { claimed: 1_000_000 })
    );
}

#[test]
fn a_fixed_width_count_the_buffer_can_hold_is_accepted() {
    let mut bytes = count_only(2);
    bytes.extend_from_slice(&[0_u8; 16]);
    let mut reader = Reader::new(&bytes);

    assert_eq!(reader.read_array_len_bounded(8), Ok(2));
}

#[test]
fn a_count_exactly_filling_the_buffer_is_accepted() {
    let mut bytes = count_only(4);
    bytes.extend_from_slice(&[0_u8; 4]);
    let mut reader = Reader::new(&bytes);

    assert_eq!(reader.read_array_len_bounded(1), Ok(4));
}

#[test]
fn one_element_past_the_buffer_is_refused() {
    let mut bytes = count_only(5);
    bytes.extend_from_slice(&[0_u8; 4]);
    let mut reader = Reader::new(&bytes);

    assert_eq!(reader.read_array_len_bounded(1), Err(CodecError::LengthOutOfRange { claimed: 5 }));
}

#[test]
fn an_unknown_width_count_is_held_against_the_element_budget() {
    let over_budget = u32::try_from(MAX_UNBOUNDED_ARRAY_ELEMENTS).unwrap_or(u32::MAX);
    let bytes = count_only(over_budget.saturating_add(1));
    let mut reader = Reader::new(&bytes);

    assert!(matches!(reader.read_array_len_bounded(0), Err(CodecError::LengthOutOfRange { .. })));
}

#[test]
fn an_unknown_width_count_inside_the_budget_is_accepted() {
    let bytes = count_only(16);
    let mut reader = Reader::new(&bytes);

    assert_eq!(reader.read_array_len_bounded(0), Ok(16));
}

#[test]
fn a_zero_count_is_accepted_for_every_width() {
    for width in [0_usize, 1, 4, 8] {
        let bytes = count_only(0);
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_array_len_bounded(width), Ok(0), "width {width}");
    }
}

#[test]
fn a_truncated_count_field_is_refused() {
    let bytes = [1_u8, 2, 3];
    let mut reader = Reader::new(&bytes);

    assert!(matches!(reader.read_array_len_bounded(1), Err(CodecError::Truncated { .. })));
}

#[test]
fn a_refused_count_leaves_the_cursor_before_the_count_field() {
    let bytes = count_only(1_000_000);
    let mut reader = Reader::new(&bytes);

    let _ = reader.read_array_len_bounded(8);
    assert_eq!(reader.remaining(), bytes.len());
}
