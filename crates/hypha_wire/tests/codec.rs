//! `Reader` refuses hostile and truncated input without losing its cursor state.

use hypha_wire::codec::{CodecError, Reader};

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn string_length_exceeding_buffer_is_an_error() {
    let buf: [u8; 4] = [100, 0, 0, 0];
    let mut reader = Reader::new(&buf);
    let position_before = reader.position();
    let result = reader.read_string();
    assert_eq!(result, Err(CodecError::LengthOutOfRange { claimed: 100 }));
    assert_eq!(reader.position(), position_before);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn truncated_scalar_read_is_an_error() {
    let buf: [u8; 2] = [0x01, 0x02];
    let mut reader = Reader::new(&buf);
    let result = reader.read_u32();
    assert_eq!(result, Err(CodecError::Truncated { needed: 4, available: 2 }));
    assert_eq!(reader.position(), 0);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn read_bool_reads_every_nonzero_byte_as_true() {
    for byte in 0..=u8::MAX {
        let buf: [u8; 1] = [byte];
        let mut reader = Reader::new(&buf);
        let value = reader.read_bool().expect("every byte is a ROS bool");
        assert_eq!(value, byte != 0, "byte {byte:#04x}");
        assert_eq!(reader.position(), 1, "byte {byte:#04x} is consumed");
    }
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn read_bool_on_an_empty_buffer_is_truncated() {
    let buf: [u8; 0] = [];
    let mut reader = Reader::new(&buf);
    let result = reader.read_bool();
    assert_eq!(result, Err(CodecError::Truncated { needed: 1, available: 0 }));
    assert_eq!(reader.position(), 0);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn read_after_error_can_be_retried_with_more_data() {
    let short: [u8; 4] = [3, 0, 0, 0];
    let mut reader = Reader::new(&short);
    assert!(reader.read_string().is_err());
    assert_eq!(reader.position(), 0);

    let full: [u8; 7] = [3, 0, 0, 0, b'a', b'b', b'c'];
    let mut reader = Reader::new(&full);
    assert_eq!(reader.read_string().expect("full string"), b"abc");
}

#[test]
fn a_time_or_duration_read_that_runs_out_after_its_seconds_leaves_the_cursor() {
    // Found by `wire_properties`: six bytes hold the seconds but not the nanoseconds.
    let buf = [0_u8; 6];
    let mut reader = Reader::new(&buf);
    assert_eq!(reader.read_time(), Err(CodecError::Truncated { needed: 4, available: 2 }));
    assert_eq!(reader.position(), 0, "a failed time read moved the cursor");
    assert_eq!(reader.read_duration(), Err(CodecError::Truncated { needed: 4, available: 2 }));
    assert_eq!(reader.position(), 0, "a failed duration read moved the cursor");
}
