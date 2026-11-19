//! Owning receive for the tests that read a C subscriber by hand, through
//! `hypha_node::safe::subscriber_receive` on the subscriber the C handle names.

use hypha_ffi::{HyphaStatus, HyphaSubscriber};
use hypha_node::safe::{sample_identity, sample_payload, subscriber_receive};

/// The send times a frame header carries.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SendTimes {
    /// Host monotonic time, in nanoseconds, at which the publisher sent the frame.
    pub(crate) transport_sent_nanos: u64,
    /// Framework time, in nanoseconds, at which the publisher sent the frame.
    pub(crate) framework_sent_nanos: u64,
}

/// One received sample, copied out of the transport sample, which is already released.
pub(crate) struct Frame {
    /// The message payload.
    pub(crate) payload: Vec<u8>,
    /// The send times of its header.
    pub(crate) sent: SendTimes,
}

/// Take the next sample of `subscriber`: `Ok(None)` when none is queued; a refused frame
/// is consumed and counted.
/// # Safety
/// `subscriber` must be a live handle from `hypha_subscribe_typed`.
pub(crate) unsafe fn receive(
    subscriber: *mut HyphaSubscriber,
) -> Result<Option<Frame>, HyphaStatus> {
    // SAFETY: the caller passes a live handle, which points to the runtime's subscriber.
    let Some(endpoint) = (unsafe { subscriber.as_ref() }) else {
        return Err(HyphaStatus::InvalidArgument);
    };
    let received = match subscriber_receive(endpoint) {
        Ok(Some(received)) => received,
        Ok(None) => return Ok(None),
        Err(error) => return Err(HyphaStatus::from(error)),
    };
    let payload = sample_payload(&received).map_err(HyphaStatus::from)?.to_vec();
    let identity = sample_identity(&received).map_err(HyphaStatus::from)?;
    let sent = SendTimes {
        transport_sent_nanos: identity.transport_sent_nanos,
        framework_sent_nanos: identity.framework_sent_nanos,
    };
    let frame = Frame { payload, sent };
    Ok(Some(frame))
}
