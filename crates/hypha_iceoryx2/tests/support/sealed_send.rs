//! Sealing for transport tests that send raw payloads and do not check the frame.

use hypha_wire::frame::FrameMetadata;

/// Metadata stamped with the steady clock now, for a payload no test validates.
#[allow(clippy::expect_used)]
pub(crate) fn metadata() -> FrameMetadata {
    let now = u64::try_from(hypha_time::steady_now().nanos()).expect("a steady time");
    FrameMetadata {
        schema_hash: 1,
        publisher_id: 1,
        sequence: 0,
        transport_sent_nanos: now,
        framework_sent_nanos: now,
    }
}
