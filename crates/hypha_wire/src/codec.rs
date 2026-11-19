//! Reading the ROS 1 primitive encoding. Writing is in [`crate::slice_writer`].

use core::fmt;

/// Maximum elements an array may declare when its element width is not fixed.
/// An element whose width is not fixed gives no length evidence of its own.
pub const MAX_UNBOUNDED_ARRAY_ELEMENTS: usize = 1 << 20;

/// An error from a `Reader` operation.
/// A `Reader` never panics; every decode failure comes back as one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    /// The read ran past the end of the buffer.
    Truncated {
        /// Byte count the read required.
        needed: usize,
        /// Byte count left in the buffer.
        available: usize,
    },
    /// A length or count field would not fit in the remaining buffer.
    LengthOutOfRange {
        /// The length or count value read from the wire.
        claimed: u32,
    },
    /// A string field did not hold valid UTF-8.
    /// Generated code that presents a field as text refuses the sample.
    InvalidUtf8,
    /// A collection is longer than the wire format can express.
    /// The wire carries a `u32` count.
    ArrayTooLong {
        /// The element count the caller supplied.
        len: usize,
    },
    /// Messages nest deeper than the decoder allows.
    /// A recursive or very deep definition is refused instead of exhausting the stack.
    NestingTooDeep {
        /// The deepest nesting the decoder accepts.
        limit: usize,
    },
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { needed, available } => {
                write!(
                    f,
                    "read ran past buffer end: needed {needed} bytes, but only {available} available"
                )
            }
            Self::LengthOutOfRange { claimed } => {
                write!(f, "field length {claimed} out of range for remaining buffer")
            }
            Self::InvalidUtf8 => f.write_str("string field is not valid UTF-8"),
            Self::ArrayTooLong { len } => {
                write!(f, "array length {len} exceeds format representation limit")
            }
            Self::NestingTooDeep { limit } => {
                write!(f, "messages nest deeper than {limit} levels")
            }
        }
    }
}

impl core::error::Error for CodecError {}

/// Result of a `Reader` operation.
pub type Result<T> = core::result::Result<T, CodecError>;

/// Reader for the ROS 1 binary encoding.
/// Every `read_*` method never panics, on any input. On error, the cursor stays where
/// the failed read started.
pub struct Reader<'a> {
    buf: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    /// Makes a reader positioned at the start of `buf`.
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, cursor: 0 }
    }

    /// Returns the number of unread bytes left in the buffer.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.cursor)
    }

    /// Returns true if no bytes are left to read.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Returns the current cursor position, in bytes from the start.
    #[must_use]
    pub fn position(&self) -> usize {
        self.cursor
    }

    /// Reads `n` raw bytes and advances the cursor by `n`.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than `n` bytes remain.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let rest = self.buf.get(self.cursor..).unwrap_or_default();
        let Some((slice, _)) = rest.split_at_checked(n) else {
            return Err(CodecError::Truncated { needed: n, available: rest.len() });
        };
        self.cursor = self.cursor.saturating_add(n);
        Ok(slice)
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let bytes = self.read_bytes(N)?;
        bytes.try_into().map_err(|_| CodecError::Truncated { needed: N, available: bytes.len() })
    }

    /// Reads one `bool`, as ROS 1 reads it: 0 is `false`, every other byte is `true`.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if the buffer is empty.
    pub fn read_bool(&mut self) -> Result<bool> {
        self.read_u8().map(|byte| byte != 0)
    }

    /// Reads one `u8`.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if the buffer is empty.
    pub fn read_u8(&mut self) -> Result<u8> {
        self.read_array::<1>().map(|[byte]| byte)
    }

    /// Reads one `i8`.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if the buffer is empty.
    pub fn read_i8(&mut self) -> Result<i8> {
        self.read_array().map(i8::from_le_bytes)
    }

    /// Reads one `u16`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 2 bytes remain.
    pub fn read_u16(&mut self) -> Result<u16> {
        self.read_array().map(u16::from_le_bytes)
    }

    /// Reads one `i16`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 2 bytes remain.
    pub fn read_i16(&mut self) -> Result<i16> {
        self.read_array().map(i16::from_le_bytes)
    }

    /// Reads one `u32`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 4 bytes remain.
    pub fn read_u32(&mut self) -> Result<u32> {
        self.read_array().map(u32::from_le_bytes)
    }

    /// Reads one `i32`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 4 bytes remain.
    pub fn read_i32(&mut self) -> Result<i32> {
        self.read_array().map(i32::from_le_bytes)
    }

    /// Reads one `u64`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 8 bytes remain.
    pub fn read_u64(&mut self) -> Result<u64> {
        self.read_array().map(u64::from_le_bytes)
    }

    /// Reads one `i64`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 8 bytes remain.
    pub fn read_i64(&mut self) -> Result<i64> {
        self.read_array().map(i64::from_le_bytes)
    }

    /// Reads one `f32` from its IEEE-754 bits, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 4 bytes remain.
    pub fn read_f32(&mut self) -> Result<f32> {
        self.read_array().map(f32::from_le_bytes)
    }

    /// Reads one `f64` from its IEEE-754 bits, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 8 bytes remain.
    pub fn read_f64(&mut self) -> Result<f64> {
        self.read_array().map(f64::from_le_bytes)
    }

    /// Reads a length or element count: a little-endian `u32`, returned as `usize`.
    /// Not checked against the remaining buffer size.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 4 bytes remain.
    pub fn read_array_len(&mut self) -> Result<usize> {
        let before_count = self.cursor;
        let count = self.read_u32()?;
        let Ok(converted) = usize::try_from(count) else {
            self.cursor = before_count;
            return Err(CodecError::LengthOutOfRange { claimed: count });
        };
        Ok(converted)
    }

    /// Reads an element count and refuses one the buffer cannot hold.
    ///
    /// `min_element_bytes` is the smallest byte count of one element on the wire. Pass
    /// `0` when the width is not known; the count is then held against
    /// [`MAX_UNBOUNDED_ARRAY_ELEMENTS`]. Call this before reserving for the count.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if the count field itself does not fit.
    /// Returns `CodecError::LengthOutOfRange` when the count exceeds what the
    /// remaining bytes or the element budget allow; the cursor is left before the count.
    pub fn read_array_len_bounded(&mut self, min_element_bytes: usize) -> Result<usize> {
        let before_count = self.cursor;
        let count = self.read_array_len()?;
        let width_is_fixed = min_element_bytes > 0;
        let fits = if width_is_fixed {
            count.checked_mul(min_element_bytes).is_some_and(|needed| needed <= self.remaining())
        } else {
            count <= MAX_UNBOUNDED_ARRAY_ELEMENTS
        };
        if !fits {
            self.cursor = before_count;
            let claimed = u32::try_from(count).unwrap_or(u32::MAX);
            return Err(CodecError::LengthOutOfRange { claimed });
        }
        Ok(count)
    }

    /// Reads a ROS string: a `u32` byte count, then that many bytes, borrowed. No UTF-8 check.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if the count field itself does not
    /// fit. Returns `CodecError::LengthOutOfRange` if the claimed byte
    /// count exceeds the bytes left; the cursor is left before the count field.
    pub fn read_string(&mut self) -> Result<&'a [u8]> {
        let before_count = self.cursor;
        let claimed = self.read_u32()?;
        let fits = usize::try_from(claimed).ok().filter(|&len| len <= self.remaining());
        let Some(len) = fits else {
            self.cursor = before_count;
            return Err(CodecError::LengthOutOfRange { claimed });
        };
        self.read_bytes(len)
    }

    /// Reads a ROS `Time`: `secs` then `nsecs`, each a little-endian `u32`.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 8 bytes remain; the cursor does not move.
    pub fn read_time(&mut self) -> Result<(u32, u32)> {
        let before = self.cursor;
        let secs = self.read_u32()?;
        let nsecs = self.read_u32().inspect_err(|_| self.cursor = before)?;
        Ok((secs, nsecs))
    }

    /// Reads a ROS `Duration`: `secs` then `nsecs`, each a little-endian `i32`.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 8 bytes remain; the cursor does not move.
    pub fn read_duration(&mut self) -> Result<(i32, i32)> {
        let before = self.cursor;
        let secs = self.read_i32()?;
        let nsecs = self.read_i32().inspect_err(|_| self.cursor = before)?;
        Ok((secs, nsecs))
    }
}
