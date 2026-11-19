//! Turning a C ABI status into the crate's error string.

use crate::sdk::ffi;

/// The name of one ABI status code, for an error message a person reads.
fn status_name(status: i32) -> &'static str {
    match status {
        ffi::HYPHA_STATUS_OK => "ok",
        ffi::HYPHA_STATUS_INVALID_ARGUMENT => "invalid argument",
        ffi::HYPHA_STATUS_INVALID_TOPIC => "invalid topic",
        ffi::HYPHA_STATUS_TOO_LARGE => "too large",
        ffi::HYPHA_STATUS_DISCONNECTED => "disconnected",
        ffi::HYPHA_STATUS_BACKEND => "backend failure",
        ffi::HYPHA_STATUS_PANIC => "callback threw",
        ffi::HYPHA_STATUS_INVALID_STATE => "invalid state",
        ffi::HYPHA_STATUS_NOT_AVAILABLE => "not available",
        ffi::HYPHA_STATUS_INVALID_SCHEMA => "invalid schema",
        ffi::HYPHA_STATUS_RESOURCE_LIMIT => "resource limit",
        ffi::HYPHA_STATUS_QOS_MISMATCH => "QoS mismatch",
        _ => "unknown status",
    }
}

/// One error message that names both what failed and why.
pub(crate) fn status_error(what: &str, status: i32) -> String {
    format!("{what}: {} (status {status})", status_name(status))
}

/// [`status_error`] for a refusal of the safe runtime API.
pub(crate) fn named_error(what: &str, error: ffi::Error) -> String {
    status_error(what, ffi::status_code(error))
}

/// One error message that names what failed and the status number only.
pub(crate) fn numbered_error(what: impl std::fmt::Display, error: ffi::Error) -> String {
    format!("{what} (status {})", ffi::status_code(error))
}
