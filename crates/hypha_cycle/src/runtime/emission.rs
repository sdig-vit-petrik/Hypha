//! The output limits of one step: which messages a step may stage.

use super::{OutputBinding, STATUS_INVALID_ARGUMENT, STATUS_RESOURCE_LIMIT, STATUS_TOO_LARGE};

/// The output limits of one step, which admits each message as the step stages it.
///
/// A message over its output's count gets `STATUS_RESOURCE_LIMIT`, one over its
/// payload limit `STATUS_TOO_LARGE`, and an unknown output `STATUS_INVALID_ARGUMENT`.
pub struct EmissionBudget<'a> {
    outputs: &'a [OutputBinding],
    counts: &'a mut [u32],
}

impl EmissionBudget<'static> {
    /// The budget of a step with no output: every message is refused as of an unknown
    /// output. A step test's services run in it (`hypha::testing::StepHarness`).
    #[must_use]
    pub fn without_outputs() -> Self {
        Self { outputs: &[], counts: &mut [] }
    }
}

impl<'a> EmissionBudget<'a> {
    pub(super) fn new(outputs: &'a [OutputBinding], counts: &'a mut Vec<u32>) -> Self {
        counts.resize(outputs.len(), 0);
        counts.fill(0);
        Self { outputs, counts }
    }

    /// Count one message of `len` payload bytes on `output`.
    ///
    /// # Errors
    /// The status code of the limit the message breaks, as documented on the type.
    pub fn admit(&mut self, output: u32, len: usize) -> Result<(), i32> {
        let index = usize::try_from(output).map_err(|_| STATUS_INVALID_ARGUMENT)?;
        let limits = self.outputs.get(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        let count = self.counts.get_mut(index).ok_or(STATUS_INVALID_ARGUMENT)?;
        if *count >= limits.max_messages_per_iteration {
            return Err(STATUS_RESOURCE_LIMIT);
        }
        if len > limits.max_payload_bytes {
            return Err(STATUS_TOO_LARGE);
        }
        *count = count.checked_add(1).ok_or(STATUS_RESOURCE_LIMIT)?;
        Ok(())
    }
}
