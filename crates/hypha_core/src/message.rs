//! What every message type provides, whatever encodes it.

use hypha_wire::codec::CodecError;

/// What the frame header carries, so a receiver can refuse the wrong type.
pub trait MessageIdentity {
    /// The full type name, `package/Name`.
    const TYPE_NAME: &'static str;

    /// A hash of the schema; a receiver drops a sample that does not match. The ROS 1
    /// encoding puts its message `md5` here.
    const SCHEMA_HASH: &'static str;
}

/// A message that writes itself into a caller-supplied buffer.
///
/// The buffer is lent by the transport, so the send path copies nothing.
pub trait EncodeInto: MessageIdentity {
    /// Exactly how many bytes [`EncodeInto::encode_into`] writes.
    ///
    /// A publisher borrows this many bytes before it writes, so the answer must be exact.
    fn encoded_len(&self) -> usize;

    /// Write the message into `buffer` and return how many bytes were used.
    ///
    /// # Errors
    /// Returns an error when `buffer` is smaller than
    /// [`EncodeInto::encoded_len`].
    fn encode_into(&self, buffer: &mut [u8]) -> Result<usize, CodecError>;
}

/// A message that reads back as an owned value.
pub trait DecodeOwned: MessageIdentity + Sized {
    /// Read one message from `bytes`.
    ///
    /// # Errors
    /// Returns an error when the bytes run out or a field is malformed.
    fn decode_from(bytes: &[u8]) -> Result<Self, CodecError>;
}
