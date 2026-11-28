//! What a binding reports of its held value, and the verdict of one value at an instant.

use super::clock::value_usable_within;
use super::{BindingCapture, InputVerdict};

/// The facts of one binding's held report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldReport {
    /// Samples the source had accepted at the capture. Wraps.
    pub received_total: u64,
    /// Usable samples that became the held sample. Wraps.
    pub delivered_total: u64,
    /// Sequence of the held sample, when one exists.
    pub held_sequence: Option<u64>,
}

/// The held report of `capture`.
#[must_use]
pub fn held_report(capture: &BindingCapture) -> HeldReport {
    HeldReport {
        received_total: capture.received_total(),
        delivered_total: capture.delivered_total(),
        held_sequence: capture.held().map(|sample| sample.sequence),
    }
}

/// Judge the newest value of one input at `at_nanos` on the host monotonic
/// clock. A zero silence limit keeps every earlier value usable.
#[must_use]
pub fn judge_value(silence_nanos: u64, sent_nanos: Option<u64>, at_nanos: u64) -> InputVerdict {
    let Some(sent) = sent_nanos else {
        return InputVerdict::NeverArrived;
    };
    if value_usable_within(silence_nanos, sent, at_nanos) {
        InputVerdict::Usable
    } else {
        InputVerdict::Stale
    }
}
