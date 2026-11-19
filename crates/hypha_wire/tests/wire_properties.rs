//! Property tests of the codec: what `SliceWriter` writes, `Reader` reads back; and no
//! input, however malformed, makes a reader panic or claim more than its bytes hold.

use hypha_wire::codec::{CodecError, MAX_UNBOUNDED_ARRAY_ELEMENTS, Reader};
use hypha_wire::slice_writer::SliceWriter;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Shrunk failures are kept here, relative to the crate directory, and replayed first.
const REGRESSIONS: &str = "proptest-regressions/wire_properties.txt";

/// Miri interprets every step: a handful of cases still walks each code path.
const MIRI_CASES: u32 = 4;

fn config(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases: if cfg!(miri) { MIRI_CASES } else { cases },
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(REGRESSIONS))),
        ..ProptestConfig::default()
    }
}

/// One value of every primitive, string, array-count and time codec path.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Bool(bool),
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    /// Kept as bits, so a NaN compares equal to itself.
    F32(u32),
    F64(u64),
    /// `write_string` / `read_string`.
    Text(Vec<u8>),
    /// `write_bytes` / `read_bytes`, the elements of a fixed array.
    Raw(Vec<u8>),
    Time(u32, u32),
    Duration(i32, i32),
    /// `write_array_len` / `read_array_len`.
    ArrayLen(u32),
}

fn value() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<u8>().prop_map(Value::U8),
        any::<i8>().prop_map(Value::I8),
        any::<u16>().prop_map(Value::U16),
        any::<i16>().prop_map(Value::I16),
        any::<u32>().prop_map(Value::U32),
        any::<i32>().prop_map(Value::I32),
        any::<u64>().prop_map(Value::U64),
        any::<i64>().prop_map(Value::I64),
        any::<u32>().prop_map(Value::F32),
        any::<u64>().prop_map(Value::F64),
        vec(any::<u8>(), 0..24).prop_map(Value::Text),
        vec(any::<u8>(), 0..24).prop_map(Value::Raw),
        (any::<u32>(), any::<u32>()).prop_map(|(secs, nsecs)| Value::Time(secs, nsecs)),
        (any::<i32>(), any::<i32>()).prop_map(|(secs, nsecs)| Value::Duration(secs, nsecs)),
        any::<u32>().prop_map(Value::ArrayLen),
    ]
}

/// The bytes `value` occupies on the wire.
fn wire_len(value: &Value) -> usize {
    match value {
        Value::Bool(_) | Value::U8(_) | Value::I8(_) => 1,
        Value::U16(_) | Value::I16(_) => 2,
        Value::U32(_) | Value::I32(_) | Value::F32(_) | Value::ArrayLen(_) => 4,
        Value::U64(_)
        | Value::I64(_)
        | Value::F64(_)
        | Value::Time(_, _)
        | Value::Duration(_, _) => 8,
        Value::Text(bytes) => bytes.len().saturating_add(4),
        Value::Raw(bytes) => bytes.len(),
    }
}

fn total_len(values: &[Value]) -> usize {
    values.iter().map(wire_len).fold(0, usize::saturating_add)
}

fn write(writer: &mut SliceWriter<'_>, value: &Value) -> Result<(), CodecError> {
    match value {
        Value::Bool(v) => writer.write_bool(*v),
        Value::U8(v) => writer.write_u8(*v),
        Value::I8(v) => writer.write_i8(*v),
        Value::U16(v) => writer.write_u16(*v),
        Value::I16(v) => writer.write_i16(*v),
        Value::U32(v) => writer.write_u32(*v),
        Value::I32(v) => writer.write_i32(*v),
        Value::U64(v) => writer.write_u64(*v),
        Value::I64(v) => writer.write_i64(*v),
        Value::F32(bits) => writer.write_f32(f32::from_bits(*bits)),
        Value::F64(bits) => writer.write_f64(f64::from_bits(*bits)),
        Value::Text(bytes) => writer.write_string(bytes),
        Value::Raw(bytes) => writer.write_bytes(bytes),
        Value::Time(secs, nsecs) => writer.write_time(*secs, *nsecs),
        Value::Duration(secs, nsecs) => writer.write_duration(*secs, *nsecs),
        Value::ArrayLen(count) => {
            let len = usize::try_from(*count).map_err(|_| CodecError::ArrayTooLong { len: 0 })?;
            writer.write_array_len(len)
        }
    }
}

/// Reads one value of the same kind as `expected`, the way a decoder that knows the
/// definition would.
fn read(reader: &mut Reader<'_>, expected: &Value) -> Result<Value, CodecError> {
    Ok(match expected {
        Value::Bool(_) => Value::Bool(reader.read_bool()?),
        Value::U8(_) => Value::U8(reader.read_u8()?),
        Value::I8(_) => Value::I8(reader.read_i8()?),
        Value::U16(_) => Value::U16(reader.read_u16()?),
        Value::I16(_) => Value::I16(reader.read_i16()?),
        Value::U32(_) => Value::U32(reader.read_u32()?),
        Value::I32(_) => Value::I32(reader.read_i32()?),
        Value::U64(_) => Value::U64(reader.read_u64()?),
        Value::I64(_) => Value::I64(reader.read_i64()?),
        Value::F32(_) => Value::F32(reader.read_f32()?.to_bits()),
        Value::F64(_) => Value::F64(reader.read_f64()?.to_bits()),
        Value::Text(_) => Value::Text(reader.read_string()?.to_vec()),
        Value::Raw(bytes) => Value::Raw(reader.read_bytes(bytes.len())?.to_vec()),
        Value::Time(_, _) => {
            let (secs, nsecs) = reader.read_time()?;
            Value::Time(secs, nsecs)
        }
        Value::Duration(_, _) => {
            let (secs, nsecs) = reader.read_duration()?;
            Value::Duration(secs, nsecs)
        }
        Value::ArrayLen(_) => {
            let count = reader.read_array_len()?;
            Value::ArrayLen(
                u32::try_from(count).map_err(|_| CodecError::ArrayTooLong { len: count })?,
            )
        }
    })
}

/// Encodes `values` into a buffer of exactly their wire length.
fn encode(values: &[Value]) -> Result<Vec<u8>, TestCaseError> {
    let mut buffer = vec![0_u8; total_len(values)];
    let mut writer = SliceWriter::new(&mut buffer);
    for value in values {
        write(&mut writer, value)
            .map_err(|error| TestCaseError::fail(format!("{value:?} does not fit: {error}")))?;
    }
    prop_assert!(writer.is_full(), "the values fill the buffer they were sized for");
    Ok(buffer)
}

/// Reads `expected` from `bytes` in order until the first error, checking that a read
/// that succeeds advances by its own length and one that fails does not move the cursor.
fn decode_prefix(bytes: &[u8], expected: &[Value]) -> Result<Vec<Value>, TestCaseError> {
    let mut reader = Reader::new(bytes);
    let mut decoded = Vec::new();
    for template in expected {
        let before = reader.position();
        let Ok(value) = read(&mut reader, template) else {
            prop_assert_eq!(
                reader.position(),
                before,
                "a failed {:?} read moved the cursor",
                template
            );
            break;
        };
        prop_assert_eq!(reader.position(), before.saturating_add(wire_len(&value)));
        prop_assert_eq!(reader.position().saturating_add(reader.remaining()), bytes.len());
        decoded.push(value);
    }
    Ok(decoded)
}

/// One `Reader` call with the arguments a hostile decoder could pass.
#[derive(Debug, Clone)]
enum Read {
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
    Text,
    Bytes(usize),
    Time,
    Duration,
    ArrayLen,
    ArrayLenBounded(usize),
}

fn read_call() -> impl Strategy<Value = Read> {
    prop_oneof![
        Just(Read::Bool),
        Just(Read::U8),
        Just(Read::I8),
        Just(Read::U16),
        Just(Read::I16),
        Just(Read::U32),
        Just(Read::I32),
        Just(Read::U64),
        Just(Read::I64),
        Just(Read::F32),
        Just(Read::F64),
        Just(Read::Text),
        prop_oneof![0..80_usize, Just(usize::MAX)].prop_map(Read::Bytes),
        Just(Read::Time),
        Just(Read::Duration),
        Just(Read::ArrayLen),
        prop_oneof![0..16_usize, Just(usize::MAX)].prop_map(Read::ArrayLenBounded),
    ]
}

/// Runs one call and returns the bytes it says it consumed, checking what the result
/// claims against the bytes the reader had.
fn run_read(
    reader: &mut Reader<'_>,
    call: &Read,
) -> Result<Result<usize, CodecError>, TestCaseError> {
    let available = reader.remaining();
    Ok(match call {
        Read::Bool => reader.read_bool().map(|_| 1),
        Read::U8 => reader.read_u8().map(|_| 1),
        Read::I8 => reader.read_i8().map(|_| 1),
        Read::U16 => reader.read_u16().map(|_| 2),
        Read::I16 => reader.read_i16().map(|_| 2),
        Read::U32 => reader.read_u32().map(|_| 4),
        Read::I32 => reader.read_i32().map(|_| 4),
        Read::U64 => reader.read_u64().map(|_| 8),
        Read::I64 => reader.read_i64().map(|_| 8),
        Read::F32 => reader.read_f32().map(|_| 4),
        Read::F64 => reader.read_f64().map(|_| 8),
        Read::Time => reader.read_time().map(|_| 8),
        Read::Duration => reader.read_duration().map(|_| 8),
        Read::ArrayLen => reader.read_array_len().map(|_| 4),
        Read::Text => match reader.read_string() {
            Ok(text) => {
                prop_assert!(text.len() <= available.saturating_sub(4));
                Ok(text.len().saturating_add(4))
            }
            Err(error) => Err(error),
        },
        Read::Bytes(count) => match reader.read_bytes(*count) {
            Ok(bytes) => {
                prop_assert_eq!(bytes.len(), *count);
                Ok(*count)
            }
            Err(error) => Err(error),
        },
        Read::ArrayLenBounded(min_element_bytes) => {
            match reader.read_array_len_bounded(*min_element_bytes) {
                Ok(count) => {
                    // The count may be reserved for: the bytes left must hold it.
                    let left = available.saturating_sub(4);
                    if *min_element_bytes == 0 {
                        prop_assert!(count <= MAX_UNBOUNDED_ARRAY_ELEMENTS);
                    } else {
                        let needed = count.checked_mul(*min_element_bytes);
                        prop_assert!(needed.is_some_and(|needed| needed <= left));
                    }
                    Ok(4)
                }
                Err(error) => Err(error),
            }
        }
    })
}

/// Bytes for a reader: random, or small counts and lengths among random bytes.
fn hostile_bytes() -> impl Strategy<Value = Vec<u8>> {
    let chunk = prop_oneof![
        3 => (0..80_u32).prop_map(|count| count.to_le_bytes().to_vec()),
        1 => prop_oneof![Just(u32::MAX), Just(1 << 20), Just((1 << 20) + 1)]
            .prop_map(|count: u32| count.to_le_bytes().to_vec()),
        3 => vec(any::<u8>(), 0..9),
    ];
    prop_oneof![vec(any::<u8>(), 0..64), vec(chunk, 0..16).prop_map(|chunks| chunks.concat())]
}

proptest! {
    #![proptest_config(config(512))]

    #[test]
    fn every_value_written_reads_back_unchanged(values in vec(value(), 0..32)) {
        let bytes = encode(&values)?;
        let decoded = decode_prefix(&bytes, &values)?;
        prop_assert_eq!(decoded, values);
    }

    #[test]
    fn a_write_that_does_not_fit_changes_nothing(
        values in vec(value(), 1..24),
        capacity_permille in 0..1000_usize,
    ) {
        const SENTINEL: u8 = 0xA5;
        let needed = total_len(&values);
        let capacity = needed.saturating_mul(capacity_permille) / 1000;
        let mut buffer = vec![SENTINEL; capacity];
        let mut written_values = Vec::new();
        {
            let mut writer = SliceWriter::new(&mut buffer);
            for value in &values {
                let before = writer.written();
                match write(&mut writer, value) {
                    Ok(()) => {
                        prop_assert_eq!(writer.written(), before.saturating_add(wire_len(value)));
                        written_values.push(value.clone());
                    }
                    Err(error) => {
                        let is_truncated = matches!(error, CodecError::Truncated { .. });
                        prop_assert!(is_truncated, "{:?} failed with {:?}", value, error);
                        prop_assert_eq!(writer.written(), before);
                    }
                }
            }
        }
        // The values that fitted are the whole written prefix; the rest is untouched.
        let written = total_len(&written_values);
        let (prefix, rest) = buffer.split_at(written);
        prop_assert!(rest.iter().all(|&byte| byte == SENTINEL));
        prop_assert_eq!(decode_prefix(prefix, &written_values)?, written_values);
    }

    #[test]
    fn a_truncated_encoding_reads_its_whole_values_then_fails(
        values in vec(value(), 1..24),
        cut_permille in 0..1000_usize,
    ) {
        let bytes = encode(&values)?;
        let cut = bytes.len().saturating_mul(cut_permille) / 1000;
        let truncated = bytes.get(..cut).unwrap_or_default();
        let decoded = decode_prefix(truncated, &values)?;
        // Exactly the values that end at or before the cut come back, unchanged.
        let mut end = 0_usize;
        let whole = values
            .iter()
            .take_while(|value| {
                end = end.saturating_add(wire_len(value));
                end <= cut
            })
            .count();
        prop_assert_eq!(decoded.as_slice(), values.get(..whole).unwrap_or_default());
    }

    #[test]
    fn a_corrupted_encoding_reads_the_values_before_the_corruption_unchanged(
        values in vec(value(), 1..24),
        at_permille in 0..1000_usize,
        delta in 1..=255_u8,
    ) {
        let mut bytes = encode(&values)?;
        let at = bytes.len().saturating_mul(at_permille) / 1000;
        if let Some(byte) = bytes.get_mut(at) {
            *byte = byte.wrapping_add(delta);
        }
        let decoded = decode_prefix(&bytes, &values)?;
        let mut end = 0_usize;
        let before_corruption = values
            .iter()
            .take_while(|value| {
                end = end.saturating_add(wire_len(value));
                end <= at
            })
            .count();
        prop_assert!(decoded.len() >= before_corruption);
        prop_assert_eq!(decoded.get(..before_corruption), values.get(..before_corruption));
    }

    #[test]
    fn any_read_of_any_bytes_is_ok_or_an_error_that_leaves_the_cursor(
        bytes in hostile_bytes(),
        calls in vec(read_call(), 0..24),
    ) {
        let mut reader = Reader::new(&bytes);
        for call in &calls {
            let before = reader.position();
            let result = run_read(&mut reader, call)?;
            let after = reader.position();
            if let Ok(consumed) = result {
                prop_assert_eq!(after, before.saturating_add(consumed));
            } else {
                prop_assert_eq!(after, before, "{:?} failed and moved the cursor", call);
            }
            prop_assert_eq!(reader.position().saturating_add(reader.remaining()), bytes.len());
        }
    }
}
