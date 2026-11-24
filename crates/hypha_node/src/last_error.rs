//! The reason of the last refusal on the calling thread.
//!
//! The record is per thread. `hypha_cycle_create` and `hypha_cycle_commit` clear it on
//! entry. A refusal that records no reason leaves the text empty.

use std::cell::RefCell;

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Forget the recorded reason.
pub fn clear() {
    LAST_ERROR.with_borrow_mut(String::clear);
}

/// Record `reason` for `hypha_last_error` without a log line: for a refusal the caller
/// reports itself.
pub(crate) fn record(reason: &str) {
    LAST_ERROR.with_borrow_mut(|last| {
        last.clear();
        last.push_str(reason);
    });
}

/// Log `reason` as an error, record it for `hypha_last_error`, and return `status`.
pub(crate) fn refuse<S>(status: S, reason: &str) -> S {
    log::error!(target: "hypha", "hypha: {reason}");
    LAST_ERROR.with_borrow_mut(|last| {
        last.clear();
        last.push_str(reason);
    });
    status
}

/// The reason of the last refusal on this thread, empty when none was recorded since
/// the last `cycle_create` or `cycle_commit` on this thread.
#[must_use]
pub fn last_error() -> String {
    LAST_ERROR.with_borrow(String::clone)
}
