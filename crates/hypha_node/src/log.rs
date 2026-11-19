//! The logging engine across the C boundary.

/// The severity of a line. The functions take the level as a plain `u32`, so
/// an invalid value from C is counted and never undefined behaviour.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyphaLogLevel {
    /// Finest detail.
    Trace = 0,
    /// Detail for a developer.
    Debug = 1,
    /// Normal events.
    Info = 2,
    /// A fault that the node survives.
    Warn = 3,
    /// A fault that stops one operation.
    Error = 4,
    /// A fault that stops the node.
    Fatal = 5,
    /// No line has this level. As a threshold it passes nothing.
    Off = 6,
}

/// The identifier of a registered module.
pub type HyphaLogModule = u32;

/// Emit the process-wide `log.*` metrics.
pub(crate) fn emit_log_metrics(emit: &mut dyn FnMut(&str, u64)) {
    let health = hypha_log::health();
    emit("log.lines_written", health.lines_written);
    emit("log.write_failures", health.write_failures);
    emit("log.rejected_lines", health.rejected_lines);
}
