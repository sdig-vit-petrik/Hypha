//! Slice-writer integration tests.

use hypha_wire::codec::{CodecError, Reader};
use hypha_wire::slice_writer::SliceWriter;

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_exact_byte_output() {
    let mut buffer = [0u8; 19];
    {
        let mut writer = SliceWriter::new(&mut buffer);

        writer.write_u32(7).expect("write_u32 failed");
        writer.write_time(2, 3).expect("write_time failed");
        writer.write_str("map").expect("write_str failed");

        assert!(writer.is_full(), "buffer should be full");
        assert_eq!(writer.written(), 19);
        assert_eq!(writer.remaining(), 0);
    }

    let expected = [
        0x07, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x00, 0x6d, 0x61, 0x70,
    ];

    assert_eq!(&buffer[..], &expected[..], "byte output mismatch");
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_scalar_roundtrip() {
    let mut buffer = [0u8; 128];
    let mut writer = SliceWriter::new(&mut buffer);

    writer.write_bool(true).expect("write_bool failed");
    writer.write_u8(42).expect("write_u8 failed");
    writer.write_i8(-13).expect("write_i8 failed");
    writer.write_u16(1000).expect("write_u16 failed");
    writer.write_i16(-2000).expect("write_i16 failed");
    writer.write_u32(100_000).expect("write_u32 failed");
    writer.write_i32(-100_000).expect("write_i32 failed");
    writer.write_u64(1_000_000_000).expect("write_u64 failed");
    writer.write_i64(-1_000_000_000).expect("write_i64 failed");
    writer.write_f32(std::f32::consts::PI).expect("write_f32 failed");
    writer.write_f64(std::f64::consts::E).expect("write_f64 failed");

    let written_bytes = writer.written();
    let mut reader = Reader::new(&buffer[..written_bytes]);

    let bool_val = reader.read_bool().expect("read_bool failed");
    assert!(bool_val);

    let u8_val = reader.read_u8().expect("read_u8 failed");
    assert_eq!(u8_val, 42);

    let i8_val = reader.read_i8().expect("read_i8 failed");
    assert_eq!(i8_val, -13);

    let u16_val = reader.read_u16().expect("read_u16 failed");
    assert_eq!(u16_val, 1000);

    let i16_val = reader.read_i16().expect("read_i16 failed");
    assert_eq!(i16_val, -2000);

    let u32_val = reader.read_u32().expect("read_u32 failed");
    assert_eq!(u32_val, 100_000);

    let i32_val = reader.read_i32().expect("read_i32 failed");
    assert_eq!(i32_val, -100_000);

    let u64_val = reader.read_u64().expect("read_u64 failed");
    assert_eq!(u64_val, 1_000_000_000);

    let i64_val = reader.read_i64().expect("read_i64 failed");
    assert_eq!(i64_val, -1_000_000_000);

    let f32_val = reader.read_f32().expect("read_f32 failed");
    assert!((f32_val - std::f32::consts::PI).abs() < 0.001);

    let f64_val = reader.read_f64().expect("read_f64 failed");
    assert!((f64_val - std::f64::consts::E).abs() < 0.001);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_buffer_too_small_fails() {
    let mut buffer = [0u8; 3];
    let mut writer = SliceWriter::new(&mut buffer);

    let result = writer.write_u32(42);
    assert!(matches!(result, Err(CodecError::Truncated { needed: 4, available: 3 })));
    assert_eq!(writer.written(), 0, "cursor should not advance on failed write");
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_failed_write_preserves_cursor() {
    let mut buffer = [0u8; 10];
    let mut writer = SliceWriter::new(&mut buffer);

    writer.write_u32(42).expect("first write failed");
    assert_eq!(writer.written(), 4);

    let result = writer.write_u64(100);
    assert!(matches!(result, Err(CodecError::Truncated { needed: 8, available: 6 })));
    assert_eq!(writer.written(), 4, "cursor should not advance on failed write");

    writer.write_u16(500).expect("second write failed");
    assert_eq!(writer.written(), 6);
    assert_eq!(writer.remaining(), 4);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_zero_length_buffer() {
    let mut buffer = [0u8; 0];
    let mut writer = SliceWriter::new(&mut buffer);

    let result = writer.write_u8(42);
    assert!(matches!(result, Err(CodecError::Truncated { needed: 1, available: 0 })));
    assert!(writer.is_full());
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_string_insufficient_payload_space() {
    let mut buffer = [0u8; 6];
    let mut writer = SliceWriter::new(&mut buffer);

    let result = writer.write_string(b"hello");
    assert!(matches!(result, Err(CodecError::Truncated { needed: 9, .. })));
    assert_eq!(writer.written(), 0, "buffer should be unchanged");
    assert_eq!(buffer, [0, 0, 0, 0, 0, 0], "buffer should still be all zeros");
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_write_empty_bytes() {
    let mut buffer = [0u8; 10];
    let mut writer = SliceWriter::new(&mut buffer);

    writer.write_bytes(&[]).expect("write_bytes failed");
    assert_eq!(writer.written(), 0);
    assert_eq!(writer.remaining(), 10);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_remaining_and_written_consistency() {
    let mut buffer = [0u8; 20];
    let mut writer = SliceWriter::new(&mut buffer);

    assert_eq!(writer.written(), 0);
    assert_eq!(writer.remaining(), 20);
    assert_eq!(writer.written() + writer.remaining(), 20);

    writer.write_u32(42).expect("write_u32 failed");
    assert_eq!(writer.written(), 4);
    assert_eq!(writer.remaining(), 16);
    assert_eq!(writer.written() + writer.remaining(), 20);

    writer.write_u64(100).expect("write_u64 failed");
    assert_eq!(writer.written(), 12);
    assert_eq!(writer.remaining(), 8);
    assert_eq!(writer.written() + writer.remaining(), 20);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_array_too_long() {
    let mut buffer = [0u8; 100];
    let mut writer = SliceWriter::new(&mut buffer);

    let oversize = (u32::MAX as usize) + 1;
    let result = writer.write_array_len(oversize);
    assert!(matches!(result, Err(CodecError::ArrayTooLong { len }) if len == oversize));
    assert_eq!(writer.written(), 0, "cursor should not advance");
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_write_bytes_with_content() {
    let mut buffer = [0u8; 10];
    let mut writer = SliceWriter::new(&mut buffer);

    let data = [1, 2, 3, 4, 5];
    writer.write_bytes(&data).expect("write_bytes failed");

    assert_eq!(writer.written(), 5);
    assert_eq!(writer.remaining(), 5);
    assert_eq!(&buffer[..5], &data[..]);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_sequential_writes() {
    let mut buffer = [0u8; 30];
    let written_len = {
        let mut writer = SliceWriter::new(&mut buffer);

        writer.write_u8(255).expect("write_u8 failed");
        writer.write_u16(65_535).expect("write_u16 failed");
        writer.write_u32(4_294_967_295).expect("write_u32 failed");

        assert_eq!(writer.written(), 7);
        assert_eq!(writer.remaining(), 23);

        writer.written()
    };

    let mut reader = Reader::new(&buffer[..written_len]);
    assert_eq!(reader.read_u8().expect("read_u8 failed"), 255);
    assert_eq!(reader.read_u16().expect("read_u16 failed"), 65_535);
    assert_eq!(reader.read_u32().expect("read_u32 failed"), 4_294_967_295);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_duration() {
    let mut buffer = [0u8; 8];
    {
        let mut writer = SliceWriter::new(&mut buffer);

        writer.write_duration(42, -1000).expect("write_duration failed");
        assert_eq!(writer.written(), 8);
    }

    let mut reader = Reader::new(&buffer[..]);
    let (secs, nsecs) = reader.read_duration().expect("read_duration failed");
    assert_eq!(secs, 42);
    assert_eq!(nsecs, -1000);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_string_empty() {
    let mut buffer = [0u8; 10];
    let written_len = {
        let mut writer = SliceWriter::new(&mut buffer);

        writer.write_str("").expect("write_str failed");
        assert_eq!(writer.written(), 4, "should write length prefix only");

        writer.written()
    };

    let mut reader = Reader::new(&buffer[..written_len]);
    let text_bytes = reader.read_string().expect("read_string failed");
    assert_eq!(text_bytes, b"");
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn test_is_full() {
    let mut buffer = [0u8; 4];
    let mut writer = SliceWriter::new(&mut buffer);

    assert!(!writer.is_full());
    writer.write_u32(123).expect("write_u32 failed");
    assert!(writer.is_full());
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn ros_time_encodes_into_a_fixed_buffer() {
    use hypha_wire::ros_time::{RosDuration, RosTime};

    let mut buffer = [0_u8; 16];
    let written = {
        let mut writer = SliceWriter::new(&mut buffer);
        RosTime::new(2, 3).encode(&mut writer).expect("time fits");
        RosDuration::new(-1, -2).encode(&mut writer).expect("duration fits");
        writer.written()
    };

    let mut reader = Reader::new(&buffer[..written]);
    assert_eq!(RosTime::decode(&mut reader), Ok(RosTime::new(2, 3)));
    assert_eq!(RosDuration::decode(&mut reader), Ok(RosDuration::new(-1, -2)));
    assert!(reader.is_empty());
}

#[test]
fn ros_time_encoding_refuses_a_short_buffer_without_advancing() {
    use hypha_wire::codec::CodecError;
    use hypha_wire::ros_time::RosTime;

    let mut buffer = [0xa5_u8; 7];
    {
        let mut writer = SliceWriter::new(&mut buffer);
        assert_eq!(
            RosTime::new(2, 3).encode(&mut writer),
            Err(CodecError::Truncated { needed: 8, available: 7 })
        );
        assert_eq!(writer.written(), 0);
    }
    assert_eq!(buffer, [0xa5; 7]);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn point_matches_ground_truth_bytes() {
    let mut buffer = [0_u8; 16];
    let written = {
        let mut writer = SliceWriter::new(&mut buffer);
        writer.write_f64(1.0).expect("x fits");
        writer.write_f64(-2.5).expect("y fits");
        writer.written()
    };
    let expected = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04,
        0xc0,
    ];
    assert_eq!(&buffer[..written], &expected);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn header_with_float_array_matches_ground_truth_bytes() {
    let mut buffer = [0_u8; 32];
    let written = {
        let mut writer = SliceWriter::new(&mut buffer);
        writer.write_u32(1).expect("sequence fits");
        writer.write_time(10, 20).expect("stamp fits");
        writer.write_str("base").expect("frame id fits");
        writer.write_array_len(2).expect("array length fits");
        writer.write_f32(1.5).expect("first value fits");
        writer.write_f32(2.5).expect("second value fits");
        writer.written()
    };
    let expected = [
        0x01, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00,
        0x00, 0x62, 0x61, 0x73, 0x65, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x3f, 0x00, 0x00,
        0x20, 0x40,
    ];
    assert_eq!(&buffer[..written], &expected);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn every_scalar_matches_ground_truth_bytes() {
    let mut buffer = [0_u8; 44];
    let written = {
        let mut writer = SliceWriter::new(&mut buffer);
        writer.write_bool(true).expect("true fits");
        writer.write_bool(false).expect("false fits");
        writer.write_u8(0xab).expect("u8 fits");
        writer.write_i8(-5).expect("i8 fits");
        writer.write_u16(0x1234).expect("u16 fits");
        writer.write_i16(-1234).expect("i16 fits");
        writer.write_u32(0x1234_5678).expect("u32 fits");
        writer.write_i32(-123_456).expect("i32 fits");
        writer.write_u64(0x0123_4567_89ab_cdef).expect("u64 fits");
        writer.write_i64(-1_234_567_890).expect("i64 fits");
        writer.write_f32(3.5).expect("f32 fits");
        writer.write_f64(-7.25).expect("f64 fits");
        writer.written()
    };
    let expected = [
        0x01, 0x00, 0xab, 0xfb, 0x34, 0x12, 0x2e, 0xfb, 0x78, 0x56, 0x34, 0x12, 0xc0, 0x1d, 0xfe,
        0xff, 0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01, 0x2e, 0xfd, 0x69, 0xb6, 0xff, 0xff,
        0xff, 0xff, 0x00, 0x00, 0x60, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1d, 0xc0,
    ];
    assert_eq!(written, expected.len());
    assert_eq!(&buffer[..written], &expected);
}

#[test]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn zero_length_array_has_a_four_byte_count() {
    let mut buffer = [0xff_u8; 4];
    {
        let mut writer = SliceWriter::new(&mut buffer);
        writer.write_array_len(0).expect("zero fits");
        assert_eq!(writer.written(), 4);
    }
    assert_eq!(buffer, [0, 0, 0, 0]);
}

#[test]
#[allow(clippy::expect_used)]
fn signed_duration_has_little_endian_golden_bytes() {
    let mut buffer = [0_u8; 8];
    {
        let mut writer = SliceWriter::new(&mut buffer);
        writer.write_duration(-1, -2).expect("duration fits");
        assert_eq!(writer.written(), 8);
    }
    assert_eq!(buffer, [0xff, 0xff, 0xff, 0xff, 0xfe, 0xff, 0xff, 0xff]);
}

#[test]
fn ros_duration_encoding_refuses_a_short_buffer_without_advancing() {
    use hypha_wire::codec::CodecError;
    use hypha_wire::ros_time::RosDuration;

    let mut buffer = [0xa5_u8; 7];
    {
        let mut writer = SliceWriter::new(&mut buffer);
        assert_eq!(
            RosDuration::new(-1, -2).encode(&mut writer),
            Err(CodecError::Truncated { needed: 8, available: 7 })
        );
        assert_eq!(writer.written(), 0);
    }
    assert_eq!(buffer, [0xa5; 7]);
}
