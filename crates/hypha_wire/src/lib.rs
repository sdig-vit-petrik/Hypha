//! The wire layer for hypha: a message to bytes and back. `no_std`, no OS call, no
//! transport; `alloc` only in [`integrity`].
//!
//! # The frame
//!
//! One sample on the bus is one [`frame::FrameHeader`], carried as the iceoryx2 user
//! header, and the message payload.
//!
//! ```text
//! field                  type    encoding
//! magic                  u32     FRAME_MAGIC, 0x3758_4F49 (bytes 0x49 0x4F 0x58 0x37, "IOX7")
//! flags                  u32     bit 0 = FLAG_LATCHED
//! schema_hash            u64
//! publisher_id           u64
//! sequence               u64
//! transport_sent_nanos   u64
//! framework_sent_nanos   u64
//! crc                    u32     CRC-32C over the fields above and the payload, started
//!                                from the code of the topic (frame::TopicCode)
//! ```
//!
//! Every field is little-endian. [`frame::seal`] computes the CRC and [`frame::verify`]
//! checks it. A receiver refuses any other magic, and a frame sealed for another topic. `transport_sent_nanos` is host
//! monotonic time; `framework_sent_nanos` is framework time (simulated on a sim-time
//! node, the logged time on replay). `FLAG_LATCHED` is set by the transport on every
//! sample of a latched publisher.
//!
//! The payload is the message alone; read it with [`codec::Reader`].
//!
//! # The payload encoding
//!
//! Fields follow each other with no padding.
//!
//! | definition type | bytes | encoding |
//! |-----------------|-------|----------|
//! | `bool` | 1 | written as 0 or 1; on read, 0 is false and any other byte is true, as in ROS 1 |
//! | `int8` … `int64`, `uint8` … `uint64` | 1, 2, 4, 8 | little-endian two's complement |
//! | `float32`, `float64` | 4, 8 | IEEE-754 bits, little-endian |
//! | `string` | 4 + n | `u32` byte count, then n bytes. No NUL, no UTF-8 check. |
//! | `time` | 8 | `secs` then `nsecs`, each `u32` |
//! | `duration` | 8 | `secs` then `nsecs`, each `i32` |
//! | `T[]` | 4 + elements | `u32` element count, then the elements |
//! | `T[n]` | elements | the elements only, with no count |
//!
//! MD5 here is a version check, not a security mechanism. No function here panics on
//! malformed input; each fault comes back as an error or `None`.
//!
//! # One message, end to end
//!
//! ```
//! use hypha_wire::codec::Reader;
//! use hypha_wire::slice_writer::SliceWriter;
//! use hypha_wire::frame::{seal, verify, FrameMetadata, TopicCode};
//! use hypha_wire::schema_hash_from_md5;
//!
//! const MD5: &str = "2176decaecbce78abc3b96ef049fabed";
//!
//! fn main() -> Result<(), &'static str> {
//!     let schema_hash = schema_hash_from_md5(MD5).ok_or("MD5 is a valid identity")?;
//!
//!     let mut payload = [0_u8; 19];
//!     let payload_len = {
//!         let mut writer = SliceWriter::new(&mut payload);
//!         writer.write_u32(7).map_err(|_| "the buffer is too small")?;
//!         writer.write_time(2, 3).map_err(|_| "the buffer is too small")?;
//!         writer.write_str("map").map_err(|_| "the frame id is longer than a u32 count")?;
//!         writer.written()
//!     };
//!     let payload = &payload[..payload_len];
//!
//!     let topic = TopicCode::of("/pose");
//!     let header = seal(
//!         topic,
//!         FrameMetadata { schema_hash, publisher_id: 1, sequence: 1, transport_sent_nanos: 1, framework_sent_nanos: 1 },
//!         0,
//!         payload,
//!     );
//!
//!     let Ok(parsed) = verify(topic, &header, payload) else {
//!         return Err("a frame we just sealed must verify");
//!     };
//!     assert_eq!(parsed.transport_sent_nanos(), 1);
//!     assert_eq!(parsed.schema_hash(), schema_hash);
//!
//!     let mut reader = Reader::new(parsed.payload());
//!     assert_eq!(reader.read_u32(), Ok(7));
//!     assert_eq!(reader.read_time(), Ok((2, 3)));
//!     assert_eq!(reader.read_string(), Ok(&b"map"[..]));
//!     assert!(reader.is_empty());
//!     Ok(())
//! }
//! ```
//!

#![no_std]
extern crate alloc;

pub mod codec;
pub mod frame;
pub mod integrity;
pub mod ros_time;
pub mod schema_record;
pub mod slice_writer;

/// True when `md5` is a message identity: exactly 32 lower-case hexadecimal digits.
///
/// The schema record compares the MD5 byte for byte, so upper case would be a second identity.
#[must_use]
pub fn is_valid_md5(md5: &str) -> bool {
    md5.len() == 32 && md5.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Extract the 64-bit schema hash prefix from an MD5 identity.
///
/// Returns `None` unless [`is_valid_md5`] accepts `md5`.
#[must_use]
pub fn schema_hash_from_md5(md5: &str) -> Option<u64> {
    if !is_valid_md5(md5) {
        return None;
    }
    let mut bytes = [0_u8; 8];
    let (pairs, _) = md5.as_bytes().as_chunks::<2>();
    for (slot, pair) in bytes.iter_mut().zip(pairs) {
        let digits = core::str::from_utf8(pair).ok()?;
        *slot = u8::from_str_radix(digits, 16).ok()?;
    }
    Some(u64::from_le_bytes(bytes))
}
