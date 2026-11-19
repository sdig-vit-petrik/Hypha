//! The status of a wrapper call.

use hypha_node::status::HyphaStatus;
pub(crate) use hypha_node::status::status_of;

/// Run `body`, and report its `Err` as the status of the call.
pub(crate) fn try_guard<F: FnOnce() -> Result<(), HyphaStatus>>(body: F) -> HyphaStatus {
    match body() {
        Ok(()) => HyphaStatus::Ok,
        Err(status) => status,
    }
}
