//! RFC 1321 MD5, and the ROS 1 identity of a message definition text.

use alloc::string::String;
use alloc::vec::Vec;

use crate::definition::parse_definition;
use crate::resolve::message_identity;
use crate::spec::TypeName;

/// One 64-byte block worth of message schedule words, little-endian.
type Block = [u32; 16];

/// Per-round word index into the 16-word block, one entry per round 0..64.
const MSG_INDEX: [usize; 64] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 1, 6, 11, 0, 5, 10, 15, 4, 9, 14, 3, 8,
    13, 2, 7, 12, 5, 8, 11, 14, 1, 4, 7, 10, 13, 0, 3, 6, 9, 12, 15, 2, 0, 7, 14, 5, 12, 3, 10, 1,
    8, 15, 6, 13, 4, 11, 2, 9,
];

/// Per-round left-rotate amount, one entry per round 0..64.
const SHIFT: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// The RFC 1321 sine-derived additive constants, one per round 0..64.
const K: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

/// The MD5 initial chaining value, from RFC 1321.
const INITIAL_STATE: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

/// Computes the raw 16-byte MD5 digest of `bytes`.
#[must_use]
pub fn md5_digest(bytes: &[u8]) -> [u8; 16] {
    let padded = pad_message(bytes);
    let mut state = INITIAL_STATE;
    for block in padded.as_chunks::<64>().0 {
        state = process_block(state, block);
    }
    digest_bytes(state)
}

/// Computes the MD5 digest of `bytes` as 32 lowercase hex digits.
#[must_use]
pub fn md5_hex(bytes: &[u8]) -> String {
    md5_digest(bytes)
        .into_iter()
        .flat_map(|byte| [hex_digit(byte >> 4), hex_digit(byte & 0x0f)])
        .collect()
}

/// Pads per RFC 1321: a `0x80` byte, zeros to a 56-mod-64 boundary, then the bit length
/// as a little-endian 64-bit integer.
fn pad_message(bytes: &[u8]) -> Vec<u8> {
    let bit_length = u64::try_from(bytes.len()).map_or(u64::MAX, |length| length.wrapping_mul(8));
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_le_bytes());
    padded
}

/// Reads one 64-byte block into 16 little-endian words.
fn words_from_block(block: &[u8]) -> Block {
    let mut words: Block = [0u32; 16];
    for (slot, chunk) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *slot = u32::from_le_bytes(*chunk);
    }
    words
}

/// Runs the MD5 compression function over one 64-byte block.
fn process_block(state: [u32; 4], block: &[u8]) -> [u32; 4] {
    let words = words_from_block(block);
    let [state_a, state_b, state_c, state_d] = state;
    let (a, b, c, d) = (0..64usize)
        .fold((state_a, state_b, state_c, state_d), |(a, b, c, d), round| {
            round_step(a, b, c, d, &words, round)
        });

    [
        state_a.wrapping_add(a),
        state_b.wrapping_add(b),
        state_c.wrapping_add(c),
        state_d.wrapping_add(d),
    ]
}

/// One MD5 round.
// RFC 1321 names the four chaining words A, B, C, D.
#[allow(clippy::many_single_char_names)]
fn round_step(a: u32, b: u32, c: u32, d: u32, words: &Block, round: usize) -> (u32, u32, u32, u32) {
    let group = round.checked_div(16).unwrap_or(0);
    let f = match group {
        0 => (b & c) | (!b & d),
        1 => (d & b) | (!d & c),
        2 => b ^ c ^ d,
        _ => c ^ (b | !d),
    };
    let word_index = MSG_INDEX.get(round).copied().unwrap_or(0);
    let word = words.get(word_index).copied().unwrap_or(0);
    let constant = K.get(round).copied().unwrap_or(0);
    let shift = SHIFT.get(round).copied().unwrap_or(0);

    let sum = a.wrapping_add(f).wrapping_add(constant).wrapping_add(word);
    let rotated = sum.rotate_left(shift);
    let new_b = b.wrapping_add(rotated);
    (d, new_b, b, c)
}

/// Serializes the four chaining words to the 16-byte digest, each little-endian.
fn digest_bytes(state: [u32; 4]) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    for (chunk, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *chunk = word.to_le_bytes();
    }
    bytes
}

/// Renders one nibble (0..16) as a lowercase hex digit.
fn hex_digit(nibble: u8) -> char {
    char::from_digit(u32::from(nibble), 16).unwrap_or('0')
}

/// Computes a ROS 1 message's identity hash from its definition text.
///
/// The text is the type's own body, then one `MSG:` section per dependency.
/// Returns `None` when the text does not fully describe the type; never fall back to a
/// guessed hash.
#[must_use]
pub fn md5_from_definition(type_name: &str, definition_text: &str) -> Option<String> {
    let root = TypeName::parse(type_name)?;
    let specs = parse_definition(&root, definition_text).ok()?;
    message_identity(&specs, &root).ok()
}
