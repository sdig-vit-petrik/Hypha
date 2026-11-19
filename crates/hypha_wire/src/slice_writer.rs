//! Writer for the ROS 1 binary encoding into a fixed-size buffer.

use crate::codec::{CodecError, Result};

/// Writer for the ROS 1 binary encoding into a fixed-size buffer.
///
/// A write that would exceed the buffer returns `CodecError::Truncated` without
/// modifying the buffer or advancing the cursor.
pub struct SliceWriter<'a> {
    buffer: &'a mut [u8],
    cursor: usize,
}

impl<'a> SliceWriter<'a> {
    /// Makes a writer that writes into `buffer` starting at offset 0.
    #[must_use]
    pub fn new(buffer: &'a mut [u8]) -> Self {
        SliceWriter { buffer, cursor: 0 }
    }

    fn reserve(&mut self, needed: usize) -> Result<&mut [u8]> {
        let available = self.remaining();
        let rest = self.buffer.get_mut(self.cursor..).unwrap_or_default();
        let Some((slice, _)) = rest.split_at_mut_checked(needed) else {
            return Err(CodecError::Truncated { needed, available });
        };
        self.cursor = self.cursor.saturating_add(needed);
        Ok(slice)
    }

    fn write_array<const N: usize>(&mut self, bytes: [u8; N]) -> Result<()> {
        self.reserve(N)?.copy_from_slice(&bytes);
        Ok(())
    }

    /// Writes one `bool` as a single byte: 0 for false, 1 for true.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if the buffer is full.
    pub fn write_bool(&mut self, value: bool) -> Result<()> {
        self.write_array([u8::from(value)])
    }

    /// Writes one `u8`.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if the buffer is full.
    pub fn write_u8(&mut self, value: u8) -> Result<()> {
        self.write_array([value])
    }

    /// Writes one `i8`.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if the buffer is full.
    pub fn write_i8(&mut self, value: i8) -> Result<()> {
        self.write_array(value.to_le_bytes())
    }

    /// Writes one `u16`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 2 bytes remain.
    pub fn write_u16(&mut self, value: u16) -> Result<()> {
        self.write_array(value.to_le_bytes())
    }

    /// Writes one `i16`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 2 bytes remain.
    pub fn write_i16(&mut self, value: i16) -> Result<()> {
        self.write_array(value.to_le_bytes())
    }

    /// Writes one `u32`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 4 bytes remain.
    pub fn write_u32(&mut self, value: u32) -> Result<()> {
        self.write_array(value.to_le_bytes())
    }

    /// Writes one `i32`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 4 bytes remain.
    pub fn write_i32(&mut self, value: i32) -> Result<()> {
        self.write_array(value.to_le_bytes())
    }

    /// Writes one `u64`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 8 bytes remain.
    pub fn write_u64(&mut self, value: u64) -> Result<()> {
        self.write_array(value.to_le_bytes())
    }

    /// Writes one `i64`, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 8 bytes remain.
    pub fn write_i64(&mut self, value: i64) -> Result<()> {
        self.write_array(value.to_le_bytes())
    }

    /// Writes one `f32` as its IEEE-754 bits, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 4 bytes remain.
    pub fn write_f32(&mut self, value: f32) -> Result<()> {
        self.write_array(value.to_le_bytes())
    }

    /// Writes one `f64` as its IEEE-754 bits, little-endian.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 8 bytes remain.
    pub fn write_f64(&mut self, value: f64) -> Result<()> {
        self.write_array(value.to_le_bytes())
    }

    /// Writes raw bytes with no length prefix.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if the buffer lacks room for all bytes.
    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let slice = self.reserve(bytes.len())?;
        slice.copy_from_slice(bytes);
        Ok(())
    }

    /// Writes a `u32` byte count followed by the bytes themselves.
    /// Never validates the bytes and never appends a NUL terminator.
    ///
    /// # Errors
    /// Returns `CodecError::ArrayTooLong` if `bytes.len()` does not fit in a
    /// `u32`. Returns `CodecError::Truncated` if the buffer lacks room for
    /// both the length prefix and the bytes.
    pub fn write_string(&mut self, bytes: &[u8]) -> Result<()> {
        let len_count = u32::try_from(bytes.len())
            .map_err(|_| CodecError::ArrayTooLong { len: bytes.len() })?;
        let bytes_needed = 4_usize.checked_add(bytes.len()).ok_or(CodecError::Truncated {
            needed: usize::MAX,
            available: self.buffer.len().saturating_sub(self.cursor),
        })?;
        let available = self.buffer.len().saturating_sub(self.cursor);
        if bytes_needed > available {
            return Err(CodecError::Truncated { needed: bytes_needed, available });
        }
        self.write_u32(len_count)?;
        self.write_bytes(bytes)
    }

    /// Writes a `&str` the same way as `write_string`.
    ///
    /// # Errors
    /// Returns `CodecError::ArrayTooLong` if `s.len()` does not fit in a
    /// `u32`. Returns `CodecError::Truncated` if the buffer lacks room.
    pub fn write_str(&mut self, text: &str) -> Result<()> {
        self.write_string(text.as_bytes())
    }

    /// Writes a ROS 1 `Time`: `secs` then `nsecs`, each a little-endian `u32`.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 8 bytes remain.
    pub fn write_time(&mut self, secs: u32, nsecs: u32) -> Result<()> {
        self.write_word_pair(secs.to_le_bytes(), nsecs.to_le_bytes())
    }

    /// Writes a ROS 1 `Duration`: `secs` then `nsecs`, each a little-endian `i32`.
    ///
    /// # Errors
    /// Returns `CodecError::Truncated` if fewer than 8 bytes remain.
    pub fn write_duration(&mut self, secs: i32, nsecs: i32) -> Result<()> {
        self.write_word_pair(secs.to_le_bytes(), nsecs.to_le_bytes())
    }

    /// Writes two 4-byte words into one 8-byte reservation, so neither lands alone.
    fn write_word_pair(&mut self, first: [u8; 4], second: [u8; 4]) -> Result<()> {
        let (first_output, second_output) = self.reserve(8)?.split_at_mut(4);
        first_output.copy_from_slice(&first);
        second_output.copy_from_slice(&second);
        Ok(())
    }

    /// Writes a variable-length array element count as a little-endian `u32`.
    ///
    /// A fixed-length array skips this call.
    ///
    /// # Errors
    /// Returns `CodecError::ArrayTooLong` if `len` does not fit in a `u32`.
    /// Returns `CodecError::Truncated` if the buffer lacks room.
    pub fn write_array_len(&mut self, len: usize) -> Result<()> {
        let count = u32::try_from(len).map_err(|_| CodecError::ArrayTooLong { len })?;
        self.write_u32(count)
    }

    /// Returns the number of bytes written so far.
    #[must_use]
    pub fn written(&self) -> usize {
        self.cursor
    }

    /// Returns the number of bytes left to write in the buffer.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.buffer.len().saturating_sub(self.cursor)
    }

    /// Returns true if the buffer is completely full.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.remaining() == 0
    }
}
