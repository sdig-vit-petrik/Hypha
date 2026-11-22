//! Sending: borrow a buffer, fill it, hand it back.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hypha_wire::frame::FrameMetadata;

use crate::publisher::{BackendLoan, HyphaPublisher};
use crate::status::Error;

/// A borrowed buffer. Opaque to the caller.
///
/// It is released by `hypha_loan_send` or by `hypha_loan_drop`. Losing the pointer
/// holds a transport buffer until the publisher is released.
pub struct HyphaLoan {
    inner: Option<BackendLoan>,
    schema_hash: u64,
    publisher_id: u64,
    /// The publisher's sequence counter. A loan that is dropped unsent takes no number.
    sequence_counter: Arc<AtomicU64>,
    /// Set by `stamp_transport_sent`. `None` stamps the host monotonic clock.
    transport_sent_nanos: Option<u64>,
    /// Set by `hypha_loan_send`. `None` stamps the framework clock.
    framework_sent_nanos: Option<u64>,
}

/// Borrow a frame of `payload_size` payload bytes from `publisher`. A pool with no free
/// buffer drops the sample, which the subscribers count as lost.
///
/// # Errors
/// The transport's refusal, such as `PoolExhausted` or `TooLarge`; `InvalidState` for a
/// loan whose payload is not exactly `payload_size` bytes.
pub fn publisher_loan_frame(
    publisher: &HyphaPublisher,
    payload_size: usize,
) -> Result<HyphaLoan, Error> {
    let mut held = lend_frame_or_gap(publisher, payload_size)?;
    let payload = held.payload_mut().ok_or(Error::InvalidState)?;
    if payload.len() != payload_size {
        return Err(Error::InvalidState);
    }
    Ok(held)
}

/// Send a filled loan. A negative `framework_sent_nanos` stamps framework time on this
/// thread: the time of the step or the replayed callback that sends, else the clock. The
/// loan is released whatever the result.
///
/// # Errors
/// The transport's error, or `InvalidState` for a clock that cannot be stamped.
pub fn loan_send(mut loan: HyphaLoan, framework_sent_nanos: i64) -> Result<(), Error> {
    if let Ok(nanos) = u64::try_from(framework_sent_nanos) {
        loan.stamp_framework_sent(nanos);
    }
    loan.send_frame()
}

/// Borrow a frame of `payload_size` payload bytes from `endpoint`.
///
/// The frame takes its sequence number when it is sent, so a loan that is refused or
/// dropped leaves no gap.
pub(crate) fn lend_frame(
    endpoint: &HyphaPublisher,
    payload_size: usize,
) -> Result<HyphaLoan, Error> {
    let loan = endpoint.port.loan(payload_size).map_err(Error::from)?;
    Ok(HyphaLoan {
        inner: Some(loan),
        schema_hash: endpoint.schema_hash,
        publisher_id: endpoint.publisher_id,
        sequence_counter: Arc::clone(&endpoint.next_sequence),
        transport_sent_nanos: None,
        framework_sent_nanos: None,
    })
}

/// [`lend_frame`] for a caller that sends its frame at once. A pool with no free buffer
/// drops that sample, so it takes a sequence number that the subscribers count as lost.
pub(crate) fn lend_frame_or_gap(
    endpoint: &HyphaPublisher,
    payload_size: usize,
) -> Result<HyphaLoan, Error> {
    lend_frame(endpoint, payload_size).inspect_err(|error| {
        if *error == Error::PoolExhausted {
            take_sequence(&endpoint.next_sequence);
        }
    })
}

/// Advance `counter` and return the number of the frame that takes it.
pub(crate) fn take_sequence(counter: &AtomicU64) -> u64 {
    let previous = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(hypha_wire::integrity::next_sequence(current))
        })
        .unwrap_or_default();
    hypha_wire::integrity::next_sequence(previous)
}

impl HyphaLoan {
    /// The message bytes of the loan, which the caller writes the message into.
    pub fn payload_mut(&mut self) -> Option<&mut [u8]> {
        Some(self.inner.as_mut()?.as_mut_bytes())
    }

    /// Set the host monotonic send stamp, in place of the host monotonic clock.
    pub(crate) fn stamp_transport_sent(&mut self, nanos: u64) {
        self.transport_sent_nanos = Some(nanos);
    }

    /// Set the framework send stamp, in place of the framework clock.
    pub(crate) fn stamp_framework_sent(&mut self, nanos: u64) {
        self.framework_sent_nanos = Some(nanos);
    }

    /// Stamp the frame, seal its header over the payload and send it. The loan is
    /// released whatever the result, and a send that fails leaves a gap.
    pub(crate) fn send_frame(mut self) -> Result<(), Error> {
        let inner = self.inner.take().ok_or(Error::InvalidArgument)?;
        let sequence = take_sequence(&self.sequence_counter);
        let stamp = match self.transport_sent_nanos {
            Some(nanos) => nanos,
            None => {
                u64::try_from(hypha_time::steady_now().nanos()).map_err(|_| Error::InvalidState)?
            }
        };
        // Framework time on this thread: the time of the step or of the replayed callback
        // that sends, else the clock.
        let framework_stamp = match self.framework_sent_nanos {
            Some(nanos) => nanos,
            None => u64::try_from(hypha_time::framework_now().nanos())
                .map_err(|_| Error::InvalidState)?,
        };
        let metadata = FrameMetadata {
            schema_hash: self.schema_hash,
            publisher_id: self.publisher_id,
            sequence,
            transport_sent_nanos: stamp,
            framework_sent_nanos: framework_stamp,
        };
        // The transport adds the latch flag, so the sealed header covers it.
        inner.send_frame(metadata, 0).map_err(Error::from)
    }
}
