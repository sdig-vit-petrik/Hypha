//! The iceoryx2 user header of a data service: the hypha frame header.

use hypha_wire::frame::{FrameHeader, FrameMetadata};
use iceoryx2::prelude::ZeroCopySend;

/// The type name iceoryx2 records for [`Iox2FrameHeader`]: discovery lists a
/// publish-subscribe service as a topic only when its user header has it.
pub(crate) const FRAME_HEADER_TYPE_NAME: &str = "hypha::FrameHeader";

/// The frame header in the layout iceoryx2 places in shared memory.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, ZeroCopySend)]
#[type_name("hypha::FrameHeader")] // equal to FRAME_HEADER_TYPE_NAME
pub(crate) struct Iox2FrameHeader {
    magic: u32,
    flags: u32,
    schema_hash: u64,
    publisher_id: u64,
    sequence: u64,
    transport_sent_nanos: u64,
    framework_sent_nanos: u64,
    crc: u32,
    /// The message bytes: the first `payload_len` of the slice. In the four bytes that were
    /// the field `reserved` (always 0) before `IOX7`, so the layout stays 56 bytes.
    payload_len: u32,
}

// The layout in shared memory: iceoryx2 matches services by type name and size, so a
// reordered or added field would connect to an older build and be misread silently.
const _: () = assert!(
    size_of::<Iox2FrameHeader>() == 56
        && align_of::<Iox2FrameHeader>() == 8
        && core::mem::offset_of!(Iox2FrameHeader, crc) == 48
        && core::mem::offset_of!(Iox2FrameHeader, payload_len) == 52
);

impl From<FrameHeader> for Iox2FrameHeader {
    fn from(header: FrameHeader) -> Self {
        let m = header.metadata;
        Self {
            magic: header.magic,
            flags: header.flags,
            schema_hash: m.schema_hash,
            publisher_id: m.publisher_id,
            sequence: m.sequence,
            transport_sent_nanos: m.transport_sent_nanos,
            framework_sent_nanos: m.framework_sent_nanos,
            crc: header.crc,
            payload_len: header.payload_len,
        }
    }
}

impl From<Iox2FrameHeader> for FrameHeader {
    fn from(header: Iox2FrameHeader) -> Self {
        Self {
            magic: header.magic,
            flags: header.flags,
            metadata: FrameMetadata {
                schema_hash: header.schema_hash,
                publisher_id: header.publisher_id,
                sequence: header.sequence,
                transport_sent_nanos: header.transport_sent_nanos,
                framework_sent_nanos: header.framework_sent_nanos,
            },
            payload_len: header.payload_len,
            crc: header.crc,
        }
    }
}
