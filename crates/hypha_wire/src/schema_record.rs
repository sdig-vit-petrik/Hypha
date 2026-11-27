//! Schema record frame serialization and validation.

use core::fmt;

/// The record layout: a 44-byte header (magic, schema hash, three zero fields), the
/// payload, then a 4-byte CRC-32C footer over everything before it.
const RECORD_MAGIC: u32 = 0x3558_4F49;
const RECORD_HEADER_LEN: usize = 44;
const RECORD_FOOTER_LEN: usize = 4;

/// Maximum complete schema record size (64 KiB).
pub const MAX_SCHEMA_RECORD_LEN: usize = 65_536;

/// Errors when parsing or writing a schema record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaRecordError {
    /// The frame structure is invalid or corrupt.
    InvalidFrame,
    /// The data type is empty or not valid UTF-8.
    WrongType,
    /// The MD5 hash is invalid (not 32 lower-case hex characters).
    WrongMd5,
    /// The payload is not valid UTF-8.
    InvalidUtf8,
    /// The schema definition text is empty.
    EmptyDefinition,
    /// The complete record exceeds the maximum allowed size.
    OversizedRecord,
}

impl fmt::Display for SchemaRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrame => f.write_str("the frame structure is invalid or corrupt"),
            Self::WrongType => f.write_str("the data type is empty or not valid UTF-8"),
            Self::WrongMd5 => {
                f.write_str("the MD5 hash is invalid (not 32 lower-case hex characters)")
            }
            Self::InvalidUtf8 => f.write_str("the payload is not valid UTF-8"),
            Self::EmptyDefinition => f.write_str("the schema definition text is empty"),
            Self::OversizedRecord => {
                f.write_str("the complete record exceeds the maximum allowed size")
            }
        }
    }
}

impl core::error::Error for SchemaRecordError {}

/// A parsed and verified schema record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedSchemaRecord<'a> {
    data_type: &'a str,
    md5: &'a str,
    definition: &'a str,
}

impl<'a> ParsedSchemaRecord<'a> {
    /// The message type name.
    #[must_use]
    pub fn data_type(&self) -> &'a str {
        self.data_type
    }

    /// The 32-character hexadecimal MD5 identity.
    #[must_use]
    pub fn md5(&self) -> &'a str {
        self.md5
    }

    /// The UTF-8 message definition text.
    #[must_use]
    pub fn definition(&self) -> &'a str {
        self.definition
    }
}

/// Calculate the total serialized frame length for a schema record.
#[must_use]
pub fn schema_record_frame_len(
    data_type_len: usize,
    md5_len: usize,
    definition_len: usize,
) -> Option<usize> {
    let payload_len =
        2usize.checked_add(data_type_len)?.checked_add(md5_len)?.checked_add(definition_len)?;
    RECORD_HEADER_LEN.checked_add(payload_len)?.checked_add(RECORD_FOOTER_LEN)
}

/// Writes a schema record into `buffer`.
///
/// # Errors
/// Returns [`SchemaRecordError`] when any field is invalid or buffer is too small.
pub fn write_schema_record(
    buffer: &mut [u8],
    data_type: &str,
    md5: &str,
    definition: &str,
) -> Result<usize, SchemaRecordError> {
    if data_type.is_empty() {
        return Err(SchemaRecordError::WrongType);
    }
    let Ok(data_type_len_u16) = u16::try_from(data_type.len()) else {
        return Err(SchemaRecordError::WrongType);
    };
    if !crate::is_valid_md5(md5) {
        return Err(SchemaRecordError::WrongMd5);
    }
    if definition.is_empty() {
        return Err(SchemaRecordError::EmptyDefinition);
    }

    let total_len = schema_record_frame_len(data_type.len(), md5.len(), definition.len())
        .ok_or(SchemaRecordError::OversizedRecord)?;
    if total_len > MAX_SCHEMA_RECORD_LEN || total_len > buffer.len() {
        return Err(SchemaRecordError::OversizedRecord);
    }

    let schema_hash = crate::schema_hash_from_md5(md5).ok_or(SchemaRecordError::WrongMd5)?;
    let header_end = RECORD_HEADER_LEN;
    let footer_start =
        total_len.checked_sub(RECORD_FOOTER_LEN).ok_or(SchemaRecordError::InvalidFrame)?;
    let record = buffer.get_mut(..total_len).ok_or(SchemaRecordError::InvalidFrame)?;
    record
        .get_mut(0..4)
        .ok_or(SchemaRecordError::InvalidFrame)?
        .copy_from_slice(&RECORD_MAGIC.to_le_bytes());
    record
        .get_mut(4..12)
        .ok_or(SchemaRecordError::InvalidFrame)?
        .copy_from_slice(&schema_hash.to_le_bytes());
    record.get_mut(12..header_end).ok_or(SchemaRecordError::InvalidFrame)?.fill(0);

    let Some(payload_dest) = record.get_mut(header_end..footer_start) else {
        return Err(SchemaRecordError::InvalidFrame);
    };
    let cursor = put_front(payload_dest, &data_type_len_u16.to_le_bytes())?;
    let cursor = put_front(cursor, data_type.as_bytes())?;
    let cursor = put_front(cursor, md5.as_bytes())?;
    put_front(cursor, definition.as_bytes())?;

    let checksum =
        crc_fast::crc32_iscsi(record.get(..footer_start).ok_or(SchemaRecordError::InvalidFrame)?);
    record
        .get_mut(footer_start..)
        .ok_or(SchemaRecordError::InvalidFrame)?
        .copy_from_slice(&checksum.to_le_bytes());
    Ok(total_len)
}

/// Copies `src` to the front of `dest` and returns the bytes of `dest` after it.
fn put_front<'b>(dest: &'b mut [u8], src: &[u8]) -> Result<&'b mut [u8], SchemaRecordError> {
    let (head, tail) =
        dest.split_at_mut_checked(src.len()).ok_or(SchemaRecordError::InvalidFrame)?;
    head.copy_from_slice(src);
    Ok(tail)
}

/// Parses and verifies a schema record from `bytes`.
///
/// # Errors
/// Returns [`SchemaRecordError`] when the frame is malformed or invalid.
pub fn parse_schema_record(bytes: &[u8]) -> Result<ParsedSchemaRecord<'_>, SchemaRecordError> {
    if bytes.len() > MAX_SCHEMA_RECORD_LEN {
        return Err(SchemaRecordError::OversizedRecord);
    }
    let payload = parse_record_payload(bytes)?;

    if payload.len() < 2 + 32 + 1 {
        return Err(SchemaRecordError::InvalidFrame);
    }

    let (dt_len_bytes, rest) =
        payload.split_first_chunk::<2>().ok_or(SchemaRecordError::InvalidFrame)?;
    let dt_len = usize::from(u16::from_le_bytes(*dt_len_bytes));
    if dt_len == 0 {
        return Err(SchemaRecordError::WrongType);
    }

    let (dt_bytes, rest) = rest.split_at_checked(dt_len).ok_or(SchemaRecordError::InvalidFrame)?;
    let Ok(data_type) = core::str::from_utf8(dt_bytes) else {
        return Err(SchemaRecordError::WrongType);
    };

    let (md5_bytes, def_bytes) =
        rest.split_at_checked(32).ok_or(SchemaRecordError::InvalidFrame)?;
    let Ok(md5) = core::str::from_utf8(md5_bytes) else {
        return Err(SchemaRecordError::WrongMd5);
    };
    if !crate::is_valid_md5(md5) {
        return Err(SchemaRecordError::WrongMd5);
    }

    let Ok(definition) = core::str::from_utf8(def_bytes) else {
        return Err(SchemaRecordError::InvalidUtf8);
    };
    if definition.is_empty() {
        return Err(SchemaRecordError::EmptyDefinition);
    }

    Ok(ParsedSchemaRecord { data_type, md5, definition })
}

/// Check the magic and the CRC-32C of a schema record, and return its payload.
fn parse_record_payload(bytes: &[u8]) -> Result<&[u8], SchemaRecordError> {
    let footer_start = bytes
        .len()
        .checked_sub(RECORD_FOOTER_LEN)
        .filter(|&end| end >= RECORD_HEADER_LEN)
        .ok_or(SchemaRecordError::InvalidFrame)?;
    let magic = bytes.get(0..4).ok_or(SchemaRecordError::InvalidFrame)?;
    if magic != RECORD_MAGIC.to_le_bytes() {
        return Err(SchemaRecordError::InvalidFrame);
    }
    let stored = bytes.get(footer_start..).ok_or(SchemaRecordError::InvalidFrame)?;
    let checksum =
        crc_fast::crc32_iscsi(bytes.get(..footer_start).ok_or(SchemaRecordError::InvalidFrame)?);
    if stored != checksum.to_le_bytes() {
        return Err(SchemaRecordError::InvalidFrame);
    }
    bytes.get(RECORD_HEADER_LEN..footer_start).ok_or(SchemaRecordError::InvalidFrame)
}
