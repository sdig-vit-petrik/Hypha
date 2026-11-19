//! The plain values that a run loop reports: registration identifiers, the
//! outcome of a run and the reason it stopped.

use crate::port::error::PortError;

/// Identifies one registration. Never zero, and never that of another live registration.
pub type AttachmentId = u64;

/// The identifier after `previous` that is neither zero nor in use.
///
/// Identifiers wrap past zero, and a live identifier is never handed out twice. Zero
/// means "no identifier" at the C ABI. `None` means `in_use` reported more identifiers
/// than `in_use_count`.
#[must_use]
pub fn next_free_identifier(
    previous: u64,
    in_use_count: usize,
    in_use: impl Fn(u64) -> bool,
) -> Option<u64> {
    let mut candidate = previous;
    for _ in 0..=in_use_count.saturating_add(1) {
        candidate = candidate.wrapping_add(1);
        let is_free = candidate != 0 && !in_use(candidate);
        if is_free {
            return Some(candidate);
        }
    }
    None
}

/// Why a run returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The application asked for the stop.
    Requested,
    /// A bounded run reached its time limit. No stop was asked.
    Deadline,
    /// Building the native attachments failed. No callback ran.
    SetupFailed,
    /// The run failed after it started.
    RuntimeFailed,
}

/// The outcome of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunReport {
    /// Why the run returned.
    pub reason: StopReason,
    /// The failure, when the reason is not a requested stop.
    pub error: Option<PortError>,
    /// The registration the failure belongs to. `None` when it belongs to none.
    pub attachment: Option<AttachmentId>,
}

impl RunReport {
    /// The run stopped because the application asked it to.
    #[must_use]
    pub const fn requested() -> Self {
        Self { reason: StopReason::Requested, error: None, attachment: None }
    }

    /// A bounded run reached its time limit.
    #[must_use]
    pub const fn deadline() -> Self {
        Self { reason: StopReason::Deadline, error: None, attachment: None }
    }

    /// Building the native attachments failed before any callback ran.
    #[must_use]
    pub const fn setup_failed(error: PortError) -> Self {
        Self { reason: StopReason::SetupFailed, error: Some(error), attachment: None }
    }

    /// The run failed after it started.
    #[must_use]
    pub const fn runtime_failed(error: PortError, attachment: Option<AttachmentId>) -> Self {
        Self { reason: StopReason::RuntimeFailed, error: Some(error), attachment }
    }
}
