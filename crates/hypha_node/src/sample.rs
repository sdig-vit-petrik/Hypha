//! Owning received sample handle and accessors.

use std::sync::Arc;

use hypha_wire::frame::ParsedFrame;

use crate::status::Error;
use crate::subscriber::loan_budget::LoanBudget;
use crate::subscriber::{BackendSample, FrameIdentity};

/// An owning sample returned by [`crate::subscriber::subscriber_receive`].
///
/// It holds one transport sample until it is dropped, may outlive its subscriber,
/// and may be released on another thread.
pub struct HyphaReceivedSample {
    pub(crate) sample: Option<BackendSample>,
    pub(crate) budget: Arc<LoanBudget>,
}

impl Drop for HyphaReceivedSample {
    fn drop(&mut self) {
        self.sample.take();
        self.budget.release();
    }
}

/// The payload of an owning sample: the message bytes, valid while it lives.
///
/// # Errors
/// `InvalidArgument` for a sample that holds no transport sample.
pub fn sample_payload(received: &HyphaReceivedSample) -> Result<&[u8], Error> {
    let held = received.sample.as_ref().ok_or(Error::InvalidArgument)?;
    Ok(held.payload())
}

/// The identity of an owning sample, borrowed from it.
///
/// # Errors
/// `InvalidArgument` for a sample that holds no transport sample.
pub fn sample_identity(received: &HyphaReceivedSample) -> Result<FrameIdentity<'_>, Error> {
    let held = received.sample.as_ref().ok_or(Error::InvalidArgument)?;
    let header = held.header();
    let frame = ParsedFrame::accepted(&header, held.payload());
    Ok(FrameIdentity {
        topic: held.topic(),
        transport_sent_nanos: frame.transport_sent_nanos(),
        framework_sent_nanos: frame.framework_sent_nanos(),
        schema_hash: frame.schema_hash(),
    })
}
