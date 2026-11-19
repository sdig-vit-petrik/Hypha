//! The wire `Reader` driven by a byte-coded sequence of reads.
//! Input: one byte `n`, then `n` call bytes, then the bytes the reader reads.
#![no_main]

use hypha_wire::codec::{CodecError, MAX_UNBOUNDED_ARRAY_ELEMENTS, Reader};
use libfuzzer_sys::fuzz_target;

/// The argument of a call that takes one, from the top three bits of its code:
/// `scale` times 0 to 6, or `usize::MAX`, the length a hostile decoder could pass.
fn argument(code: u8, scale: usize) -> usize {
    match code >> 5 {
        7 => usize::MAX,
        bits => usize::from(bits) * scale,
    }
}

/// Runs the call `code` names and returns the bytes it says it consumed.
fn run(reader: &mut Reader<'_>, code: u8) -> Result<usize, CodecError> {
    let available = reader.remaining();
    match code % 17 {
        0 => reader.read_bool().map(|_| 1),
        1 => reader.read_u8().map(|_| 1),
        2 => reader.read_i8().map(|_| 1),
        3 => reader.read_u16().map(|_| 2),
        4 => reader.read_i16().map(|_| 2),
        5 => reader.read_u32().map(|_| 4),
        6 => reader.read_i32().map(|_| 4),
        7 => reader.read_u64().map(|_| 8),
        8 => reader.read_i64().map(|_| 8),
        9 => reader.read_f32().map(|_| 4),
        10 => reader.read_f64().map(|_| 8),
        11 => reader.read_time().map(|_| 8),
        12 => reader.read_duration().map(|_| 8),
        13 => reader.read_array_len().map(|_| 4),
        14 => reader.read_string().map(|text| {
            assert!(text.len() <= available.saturating_sub(4), "a string longer than its bytes");
            text.len() + 4
        }),
        15 => {
            let count = argument(code, 11);
            reader.read_bytes(count).map(|bytes| {
                assert_eq!(bytes.len(), count, "read_bytes returned another length");
                count
            })
        }
        _ => {
            let min_element_bytes = argument(code, 1);
            reader.read_array_len_bounded(min_element_bytes).map(|count| {
                let left = available.saturating_sub(4);
                if min_element_bytes == 0 {
                    assert!(count <= MAX_UNBOUNDED_ARRAY_ELEMENTS, "{count} unbounded elements");
                } else {
                    let needed = count.checked_mul(min_element_bytes);
                    assert!(
                        needed.is_some_and(|needed| needed <= left),
                        "{count} elements of {min_element_bytes} bytes in {left} bytes"
                    );
                }
                4
            })
        }
    }
}

fuzz_target!(|input: &[u8]| {
    let Some((&count, rest)) = input.split_first() else {
        return;
    };
    let (calls, bytes) = rest.split_at(usize::from(count).min(rest.len()));
    let mut reader = Reader::new(bytes);
    for &code in calls {
        let before = reader.position();
        let result = run(&mut reader, code);
        let after = reader.position();
        match result {
            Ok(consumed) => assert_eq!(after, before + consumed, "call {code}: cursor"),
            Err(_) => assert_eq!(after, before, "call {code} failed and moved the cursor"),
        }
        assert!(after <= bytes.len(), "the cursor passed the buffer");
        assert_eq!(after + reader.remaining(), bytes.len(), "position and remaining disagree");
    }
});
