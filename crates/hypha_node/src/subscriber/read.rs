//! Subscriber read, ownership and diagnostic C ABI accessors.

use super::{HyphaSubscriber, MODE_RECEIVE, ReceiveFailure};
use crate::sample::HyphaReceivedSample;
use crate::status::Error;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// The topic `subscriber` resolved at creation, such as `/<node>/x` for `~/x`.
#[must_use]
pub fn subscriber_topic(subscriber: &HyphaSubscriber) -> &str {
    &subscriber.topic
}

/// An owning receive: one sample, or `None` when the queue is empty. The sample stays
/// valid until it is dropped. The first receive claims the subscriber.
///
/// # Errors
/// `InvalidOperation` for a subscriber a `WaitSet` claimed, `BorrowLimit` while it holds
/// its budget of samples, and the validation or transport error of the receive.
pub fn subscriber_receive(
    endpoint: &HyphaSubscriber,
) -> Result<Option<HyphaReceivedSample>, Error> {
    endpoint.claim(MODE_RECEIVE)?;
    if !endpoint.budget.reserve() {
        endpoint.budget.refused.fetch_add(1, Ordering::Relaxed);
        return Err(Error::BorrowLimit);
    }
    match endpoint.receive_validated() {
        Ok(Some(sample)) => Ok(Some(HyphaReceivedSample {
            sample: Some(sample),
            budget: Arc::clone(&endpoint.budget),
        })),
        Ok(None) => {
            endpoint.budget.release();
            Ok(None)
        }
        Err(ReceiveFailure::Transport(Error::BorrowLimit)) => {
            endpoint.budget.release();
            endpoint.budget.refused.fetch_add(1, Ordering::Relaxed);
            Err(Error::BorrowLimit)
        }
        Err(failure) => {
            endpoint.budget.release();
            Err(failure.error())
        }
    }
}
