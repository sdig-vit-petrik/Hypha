//! Sample integrity: the frame header and its CRC-32C.
//!
//! [`seal`] computes the header of a payload for one topic, and [`verify`] checks a
//! received header against its payload and the topic it arrived on.
use core::fmt;

use crc32c::{crc32c, crc32c_append};

/// Frame magic. It changes when the header layout changes: `IOX7` (0.4.0) added
/// `payload_len`, so a frame of `IOX6` (0.2.0, 0.3.0) is refused as another magic. The
/// topic-bound CRC of 0.2.0 kept the 0.1.0 layout and magic: a 0.1.0 frame fails its CRC.
pub const FRAME_MAGIC: u32 = 0x3758_4F49;

/// The longest payload a frame can carry: `payload_len` is a `u32`.
pub const MAX_PAYLOAD_LEN: usize = u32::MAX as usize;

/// The topic a frame is sealed for, as the start value of its CRC. The topic name is
/// not sent: a frame that arrives on another topic fails its CRC.
///
/// Two topic names give the same code with a probability of 2^-32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopicCode(u32);

impl TopicCode {
    /// The code of the topic `name`: the CRC-32C of its bytes.
    #[must_use]
    pub fn of(name: &str) -> Self {
        Self(crc32c(name.as_bytes()))
    }
}

/// Flag set on every sample of a latched publisher. A subscriber that joins
/// after a sample was sent receives that sample only when this flag is set.
pub const FLAG_LATCHED: u32 = 1;

/// Metadata stored in a frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameMetadata {
    /// Schema hash prefix.
    pub schema_hash: u64,
    /// Publisher instance identifier.
    pub publisher_id: u64,
    /// Publisher sequence number.
    pub sequence: u64,
    /// Host monotonic send timestamp.
    pub transport_sent_nanos: u64,
    /// Framework-time send timestamp: sim time on a sim-time node, wall time otherwise.
    pub framework_sent_nanos: u64,
}

/// The header of one sample, sent as the iceoryx2 user header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Must equal [`FRAME_MAGIC`].
    pub magic: u32,
    /// Bit set of `FLAG_*` values.
    pub flags: u32,
    /// The sender's metadata.
    pub metadata: FrameMetadata,
    /// The bytes of the message: the first `payload_len` bytes of the transport's slice,
    /// which may be longer (a loan shrunk before its send).
    pub payload_len: u32,
    /// CRC-32C over the magic, the flags, the metadata, `payload_len` and the payload.
    pub crc: u32,
}

/// Errors raised while parsing a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The frame magic is not supported.
    UnsupportedMagic,
    /// The checksum does not match.
    CrcMismatch,
    /// `payload_len` is longer than the bytes the transport delivered.
    LengthMismatch,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMagic => f.write_str("the frame magic is not supported"),
            Self::CrcMismatch => f.write_str("the checksum does not match"),
            Self::LengthMismatch => {
                f.write_str("the frame claims more payload bytes than it carries")
            }
        }
    }
}

impl core::error::Error for ParseError {}

/// A verified frame that borrows its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedFrame<'a> {
    metadata: FrameMetadata,
    flags: u32,
    payload: &'a [u8],
}

impl<'a> ParsedFrame<'a> {
    /// Wrap a header and payload that the caller already verified with [`verify`].
    #[must_use]
    pub fn accepted(header: &FrameHeader, payload: &'a [u8]) -> Self {
        Self { metadata: header.metadata, flags: header.flags, payload }
    }

    /// Return the schema hash prefix.
    #[must_use]
    pub fn schema_hash(&self) -> u64 {
        self.metadata.schema_hash
    }

    /// Return the publisher instance identifier.
    #[must_use]
    pub fn publisher_id(&self) -> u64 {
        self.metadata.publisher_id
    }

    /// Return the sequence number.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.metadata.sequence
    }

    /// Return the transport send timestamp.
    #[must_use]
    pub fn transport_sent_nanos(&self) -> u64 {
        self.metadata.transport_sent_nanos
    }

    /// Return the framework-time send timestamp.
    #[must_use]
    pub fn framework_sent_nanos(&self) -> u64 {
        self.metadata.framework_sent_nanos
    }

    /// True when the publisher latched this sample.
    #[must_use]
    pub fn latched(&self) -> bool {
        self.flags & FLAG_LATCHED != 0
    }

    /// Return the payload bytes.
    #[must_use]
    pub fn payload(&self) -> &'a [u8] {
        self.payload
    }
}

/// Build the header of `payload` with `metadata` and `flags`, for a send on `topic`.
/// `payload_len` is `payload.len()`. A payload over [`MAX_PAYLOAD_LEN`] gets that length,
/// which no receiver accepts for it: the transport refuses such a loan first (`TooLarge`).
#[must_use]
pub fn seal(topic: TopicCode, metadata: FrameMetadata, flags: u32, payload: &[u8]) -> FrameHeader {
    let payload_len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let unsealed = FrameHeader { magic: FRAME_MAGIC, flags, metadata, payload_len, crc: 0 };
    FrameHeader { crc: integrity_code(topic, &unsealed, payload), ..unsealed }
}

/// The message bytes of a frame whose transport slice is `slice`: its first
/// `header.payload_len` bytes, or none when the header claims more than the slice holds,
/// so a reader that has not verified the frame never sees bytes no CRC covered. [`verify`]
/// refuses such a frame as [`ParseError::LengthMismatch`].
#[must_use]
pub fn message_bytes<'a>(header: &FrameHeader, slice: &'a [u8]) -> &'a [u8] {
    usize::try_from(header.payload_len).ok().and_then(|len| slice.get(..len)).unwrap_or(&[])
}

/// Check that `header` is a supported frame, sealed for `topic`, whose message is the first
/// `payload_len` bytes of `slice`. The parsed frame's payload is those bytes.
///
/// # Errors
/// [`ParseError::UnsupportedMagic`] for another magic, [`ParseError::LengthMismatch`]
/// when `payload_len` exceeds `slice`, and [`ParseError::CrcMismatch`] when any covered
/// field or payload byte differs, or the frame was sealed for another topic.
pub fn verify<'a>(
    topic: TopicCode,
    header: &FrameHeader,
    slice: &'a [u8],
) -> Result<ParsedFrame<'a>, ParseError> {
    if header.magic != FRAME_MAGIC {
        return Err(ParseError::UnsupportedMagic);
    }
    let payload = usize::try_from(header.payload_len)
        .ok()
        .and_then(|len| slice.get(..len))
        .ok_or(ParseError::LengthMismatch)?;
    if integrity_code(topic, header, payload) != header.crc {
        return Err(ParseError::CrcMismatch);
    }
    Ok(ParsedFrame::accepted(header, payload))
}

/// The bytes the CRC covers before the payload: magic, flags, the five metadata fields and
/// `payload_len`, little-endian.
const COVERED_HEADER_LEN: usize = 52;

/// CRC-32C that starts from the topic code and runs over the little-endian magic,
/// flags, metadata fields and `payload_len`, then the payload. The header bytes go in one
/// call (the CRC of a concatenation is that of its parts in order): each call has a fixed
/// cost, and a frame pays it twice, not eight times.
fn integrity_code(topic: TopicCode, header: &FrameHeader, payload: &[u8]) -> u32 {
    let m = header.metadata;
    let mut covered = [0_u8; COVERED_HEADER_LEN];
    let mut rest = put(&mut covered, header.magic.to_le_bytes());
    rest = put(rest, header.flags.to_le_bytes());
    for field in
        [m.schema_hash, m.publisher_id, m.sequence, m.transport_sent_nanos, m.framework_sent_nanos]
    {
        rest = put(rest, field.to_le_bytes());
    }
    put(rest, header.payload_len.to_le_bytes());
    crc32c_append(crc32c_append(topic.0, &covered), payload)
}

/// Write `bytes` at the front of `rest` and return the bytes after them. The caller sizes
/// the buffer for every field, so a field never falls off its end.
fn put<const N: usize>(rest: &mut [u8], bytes: [u8; N]) -> &mut [u8] {
    let (head, tail) = rest.split_at_mut(rest.len().min(N));
    head.copy_from_slice(bytes.get(..head.len()).unwrap_or_default());
    tail
}
