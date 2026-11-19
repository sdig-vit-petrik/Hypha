//! The process-wide stop request. The SIGINT and SIGTERM handler that node
//! creation installs (`hypha_sys::signal`) records it.

/// True once a stop of the process was requested. Nothing clears it: the process
/// is ending, so a node created later is stopped from the start.
pub(crate) fn stop_requested() -> bool {
    hypha_sys::signal::stop_requested()
}
