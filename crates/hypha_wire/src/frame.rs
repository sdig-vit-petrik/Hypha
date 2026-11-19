//! Sample integrity: the frame header and its CRC-32C.
//!
//! [`seal`] computes the header of a payload, and [`verify`] checks a received header
//! against its payload.
use core::fmt;

use crc32c::{crc32c, crc32c_append};

/// Version-specific frame magic. It changes when the frame format changes.
pub const FRAME_MAGIC: u32 = 0x3658_4F49;

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
    /// CRC-32C over the magic, the flags, the metadata and the payload.
    pub crc: u32,
}

/// Errors raised while parsing a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The frame magic is not supported.
    UnsupportedMagic,
    /// The checksum does not match.
    CrcMismatch,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMagic => f.write_str("the frame magic is not supported"),
            Self::CrcMismatch => f.write_str("the checksum does not match"),
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

/// Build the header of `payload` with `metadata` and `flags`.
#[must_use]
pub fn seal(metadata: FrameMetadata, flags: u32, payload: &[u8]) -> FrameHeader {
    let unsealed = FrameHeader { magic: FRAME_MAGIC, flags, metadata, crc: 0 };
    FrameHeader { crc: integrity_code(&unsealed, payload), ..unsealed }
}

/// Check that `header` is a supported frame and that it matches `payload`.
///
/// # Errors
/// [`ParseError::UnsupportedMagic`] for another magic, and
/// [`ParseError::CrcMismatch`] when any covered field or payload byte differs.
pub fn verify<'a>(header: &FrameHeader, payload: &'a [u8]) -> Result<ParsedFrame<'a>, ParseError> {
    if header.magic != FRAME_MAGIC {
        return Err(ParseError::UnsupportedMagic);
    }
    if integrity_code(header, payload) != header.crc {
        return Err(ParseError::CrcMismatch);
    }
    Ok(ParsedFrame::accepted(header, payload))
}

/// CRC-32C over the little-endian magic, flags and metadata fields, then the payload.
fn integrity_code(header: &FrameHeader, payload: &[u8]) -> u32 {
    let m = header.metadata;
    let mut crc = crc32c(&header.magic.to_le_bytes());
    crc = crc32c_append(crc, &header.flags.to_le_bytes());
    for field in
        [m.schema_hash, m.publisher_id, m.sequence, m.transport_sent_nanos, m.framework_sent_nanos]
    {
        crc = crc32c_append(crc, &field.to_le_bytes());
    }
    crc32c_append(crc, payload)
}
