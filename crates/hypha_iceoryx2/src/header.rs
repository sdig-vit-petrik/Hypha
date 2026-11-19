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
    /// Zero. It keeps the layout a whole number of eight-byte words.
    reserved: u32,
}

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
            reserved: 0,
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
            crc: header.crc,
        }
    }
}
