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

/// The sample a subscription callback is delivered, which the callback may keep.
/// Opaque to C: `HyphaDelivery::holder` names it for the call only.
pub struct HyphaDeliveryHolder {
    sample: std::cell::Cell<Option<BackendSample>>,
    budget: Arc<LoanBudget>,
}

impl HyphaDeliveryHolder {
    pub(crate) fn new(sample: BackendSample, budget: Arc<LoanBudget>) -> Self {
        Self { sample: std::cell::Cell::new(Some(sample)), budget }
    }

    /// The delivered sample, unless a callback kept it.
    pub(crate) fn sample(&self) -> Option<BackendSample> {
        self.sample.take()
    }

    /// Put back a sample [`HyphaDeliveryHolder::sample`] took.
    pub(crate) fn put_back(&self, sample: BackendSample) {
        self.sample.set(Some(sample));
    }
}

/// Keep the sample of a delivery after its callback returns: the payload stays where it
/// is, in shared memory, until the returned sample is dropped. It counts against the
/// subscriber's budget, which leaves one sample for the next delivery.
///
/// # Errors
/// `BorrowLimit`, counted as a refused loan, while the subscriber's callbacks hold their
/// budget; `InvalidState` when the sample was kept already.
pub fn delivery_retain(holder: &HyphaDeliveryHolder) -> Result<HyphaReceivedSample, Error> {
    let Some(sample) = holder.sample() else {
        return Err(Error::InvalidState);
    };
    if !holder.budget.reserve() {
        holder.budget.refused.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        holder.put_back(sample);
        return Err(Error::BorrowLimit);
    }
    Ok(HyphaReceivedSample { sample: Some(sample), budget: Arc::clone(&holder.budget) })
}
