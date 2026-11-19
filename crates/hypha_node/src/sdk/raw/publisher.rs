//! Raw publisher: frames bytes under a topic identity and sends them.

use crate::sdk::ffi;

/// Publishes raw bytes under an explicit type identity.
pub struct RawPublisher {
    pub(crate) publisher: ffi::HyphaPublisher,
}

impl RawPublisher {
    /// Publishes a payload, stamped with the framework clock.
    #[must_use = "publishing can fail when the transport refuses the buffer or the header cannot be encoded"]
    pub fn publish(&mut self, payload: &[u8]) -> bool {
        self.publish_frame(payload, None)
    }

    /// Publishes a payload stamped with `framework_sent_nanos` instead of the
    /// framework clock.
    #[must_use = "publishing can fail when the transport refuses the buffer or the header cannot be encoded"]
    pub fn publish_at_framework_nanos(
        &mut self,
        payload: &[u8],
        framework_sent_nanos: u64,
    ) -> bool {
        self.publish_frame(payload, Some(framework_sent_nanos))
    }

    fn publish_frame(&mut self, payload: &[u8], framework_sent_nanos: Option<u64>) -> bool {
        let Some(stamp) =
            framework_sent_nanos.map_or(Some(-1_i64), |nanos| i64::try_from(nanos).ok())
        else {
            return false;
        };
        let Ok(mut loan) = ffi::publisher_loan_frame(&self.publisher, payload.len()) else {
            return false;
        };
        let Some(buffer) = loan.payload_mut() else {
            return false;
        };
        let sized_for_payload = buffer.len() == payload.len();
        if !sized_for_payload {
            return false;
        }
        buffer.copy_from_slice(payload);
        ffi::loan_send(loan, stamp).is_ok()
    }

    /// The smallest queue size among the subscribers connected to this topic, or `None`.
    ///
    /// # Errors
    /// Never fails: the runtime reads the count without a refusal.
    pub fn min_subscriber_buffer(&self) -> Result<Option<usize>, String> {
        let smallest = ffi::publisher_min_subscriber_buffer(&self.publisher);
        Ok((smallest > 0).then_some(smallest))
    }
}
