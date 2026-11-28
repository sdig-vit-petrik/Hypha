//! The counters of a subscriber: what it received, rejected and lost, for the C ABI and
//! the node's metrics.

use std::sync::atomic::Ordering;

use super::HyphaSubscriber;

/// `HyphaSubscriberCounters::other_schemas` before the other streams were checked.
pub const HYPHA_NOT_CHECKED: u64 = u64::MAX;

/// The counters of one subscriber, as `hypha_subscriber_counters` writes them: nine
/// `uint64_t`, 72 bytes, owned by the caller, with nothing to destroy. Every counter but
/// `loan_budget`, `loan_held` and `other_schemas` only grows, and wraps.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HyphaSubscriberCounters {
    /// `integrity.sequence_gap`: frames refused because their sequence was not newer
    /// than the last accepted one of their publisher. A lost frame is not one.
    pub sequence_gap: u64,
    /// `integrity.invalid_frame`: frames whose layout is not an hypha frame.
    pub invalid_frame: u64,
    /// `integrity.corrupt_payload`: frames whose CRC-32C does not match.
    pub corrupt_payload: u64,
    /// `integrity.schema_mismatch`: frames whose header names another schema than the
    /// stream they arrived on, a fault of their sender.
    pub schema_mismatch: u64,
    /// `integrity.invalid_timestamp`: frames sent later than the host clock reads.
    pub invalid_timestamp: u64,
    /// `loan.budget`: the owning samples the subscriber may hold at once.
    pub loan_budget: u64,
    /// `loan.held`: the owning samples it holds now.
    pub loan_held: u64,
    /// `loan.refused`: owning receives refused because the budget was full.
    pub loan_refused: u64,
    /// `topic.other_schemas`: the other schemas (MD5s) of the subscriber's topic that a
    /// live publisher sends, as the node's housekeeping last found them, at most a second
    /// old while its default `WaitSet` runs. [`HYPHA_NOT_CHECKED`] before the first check,
    /// and for a subscriber of no single stream. A fact about the topic, not a fault count:
    /// a migration has two versions on it.
    pub other_schemas: u64,
}

/// The counters of `endpoint`. Each counter is read atomically, the nine not at one instant.
#[must_use]
pub fn subscriber_counters(endpoint: &HyphaSubscriber) -> HyphaSubscriberCounters {
    let validation = endpoint.validator.stats();
    HyphaSubscriberCounters {
        sequence_gap: validation.sequence_gap,
        invalid_frame: validation.invalid_frame,
        corrupt_payload: validation.corrupt_payload,
        schema_mismatch: validation.schema_mismatch,
        invalid_timestamp: validation.invalid_timestamp,
        // `usize` is at most 64 bits on every supported target.
        loan_budget: u64::try_from(endpoint.budget.budget.load(Ordering::Acquire)).unwrap_or(0),
        loan_held: u64::try_from(endpoint.budget.held.load(Ordering::Acquire)).unwrap_or(0),
        loan_refused: endpoint.budget.refused.load(Ordering::Acquire),
        other_schemas: endpoint
            .watch
            .as_ref()
            .and_then(crate::topic_streams::Watch::others)
            .map_or(HYPHA_NOT_CHECKED, |others| u64::try_from(others.len()).unwrap_or(u64::MAX)),
    }
}

/// Emit the `integrity.*` and `loan.*` metrics of one subscriber, in that order.
pub(crate) fn emit_subscriber_metrics(endpoint: &HyphaSubscriber, emit: &mut dyn FnMut(&str, u64)) {
    let counters = subscriber_counters(endpoint);
    emit("integrity.invalid_frame", counters.invalid_frame);
    emit("integrity.corrupt_payload", counters.corrupt_payload);
    emit("integrity.schema_mismatch", counters.schema_mismatch);
    emit("integrity.sequence_gap", counters.sequence_gap);
    emit("integrity.invalid_timestamp", counters.invalid_timestamp);
    emit("loan.budget", counters.loan_budget);
    emit("loan.held", counters.loan_held);
    emit("loan.refused", counters.loan_refused);
}
