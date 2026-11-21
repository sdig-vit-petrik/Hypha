//! Latching through the service history of iceoryx2.
//!
//! iceoryx2 hands each publisher's last sample to a new subscriber. A latched
//! publisher sets [`FLAG_LATCHED`], and a subscriber drops earlier samples without it.

use std::sync::{Arc, Weak};

use hypha_wire::frame::{FLAG_LATCHED, FrameHeader, TopicCode, verify};
use iceoryx2::port::notifier::Notifier;
use iceoryx2::port::publisher::Publisher;
use iceoryx2::port::update_connections::UpdateConnections;

use crate::header::Iox2FrameHeader;
use crate::transport::Ipc;

/// True when a sample is history of an unlatched publisher and its frame verifies.
/// A corrupt header that claims history fails verification and is delivered, so the
/// validator counts it.
pub(crate) fn is_unlatched_history(
    topic: &str,
    header: &Iox2FrameHeader,
    payload: &[u8],
    created_nanos: u64,
) -> bool {
    let frame = FrameHeader::from(*header);
    let sent_before_subscriber = frame.metadata.transport_sent_nanos < created_nanos;
    let unlatched = frame.flags & FLAG_LATCHED == 0;
    let claims_history = sent_before_subscriber && unlatched;
    // The code is computed only for a sample that claims history.
    claims_history && verify(TopicCode::of(topic), &frame, payload).is_ok()
}

/// A latched publisher, woken when a subscriber joins it.
pub(crate) struct LatchedPort {
    pub(crate) publisher: Weak<Publisher<Ipc, [u8], Iox2FrameHeader>>,
    pub(crate) notifier: Arc<Notifier<Ipc>>,
    pub(crate) topic: String,
}

impl LatchedPort {
    /// Hand the history to the subscribers that joined since the last call, and
    /// wake them. Returns false once the publisher is gone.
    pub(crate) fn replay(&mut self) -> bool {
        let Some(publisher) = self.publisher.upgrade() else {
            return false;
        };
        if publisher.update_connections().is_err() {
            log::warn!(target: "hypha/transport", "hypha_iceoryx2: latched topic {} cannot reach a new subscriber",
                    self.topic);
            return true;
        }
        if !crate::notification::notified(&self.notifier.notify()) {
            log::warn!(target: "hypha/transport", "hypha_iceoryx2: latched replay on {} was sent but its wake failed",
                    self.topic);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::is_unlatched_history;
    use crate::header::Iox2FrameHeader;
    use hypha_wire::frame::{FLAG_LATCHED, FrameHeader, FrameMetadata, TopicCode, seal};

    const TOPIC: &str = "/latch";

    const CREATED: u64 = 1_000;
    const PAYLOAD: &[u8] = b"sample";

    fn sealed(sent: u64, flags: u32) -> FrameHeader {
        let metadata = FrameMetadata {
            schema_hash: 1,
            publisher_id: 2,
            sequence: 3,
            transport_sent_nanos: sent,
            framework_sent_nanos: sent,
        };
        seal(TopicCode::of(TOPIC), metadata, flags, PAYLOAD)
    }

    #[test]
    fn an_old_verified_unlatched_frame_is_history() {
        let header = Iox2FrameHeader::from(sealed(CREATED - 1, 0));
        assert!(is_unlatched_history(TOPIC, &header, PAYLOAD, CREATED));
    }

    #[test]
    fn a_latched_or_recent_frame_is_delivered() {
        let latched = Iox2FrameHeader::from(sealed(CREATED - 1, FLAG_LATCHED));
        assert!(
            !is_unlatched_history(TOPIC, &latched, PAYLOAD, CREATED),
            "a latched sample is kept"
        );
        let recent = Iox2FrameHeader::from(sealed(CREATED + 1, 0));
        assert!(!is_unlatched_history(TOPIC, &recent, PAYLOAD, CREATED), "a live sample is kept");
    }

    /// A corrupt header whose send time reads as history is delivered and counted.
    #[test]
    fn a_corrupt_header_that_claims_history_is_delivered_to_be_counted() {
        let mut frame = sealed(CREATED + 5, 0);
        frame.metadata.transport_sent_nanos = CREATED - 1;
        let header = Iox2FrameHeader::from(frame);
        assert!(!is_unlatched_history(TOPIC, &header, PAYLOAD, CREATED));
    }
}
