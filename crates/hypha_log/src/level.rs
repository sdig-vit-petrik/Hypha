//! Severity levels.

/// A severity. A line reaches a target when its level is at or above the
/// level of the module and at or above the level of the target.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Finest detail.
    Trace = 0,
    /// Detail for a developer.
    Debug = 1,
    /// Normal events.
    Info = 2,
    /// A fault that the node can handle.
    Warn = 3,
    /// A fault that the node cannot handle.
    Error = 4,
    /// A fault after which the node cannot continue. It does not stop the process.
    Fatal = 5,
    /// No line. This is a threshold, never the level of a line.
    Off = 6,
}

impl Level {
    /// The level for `name`: `trace`, `debug`, `info`, `warn`, `error`, `fatal` or `off`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Level> {
        match name {
            "trace" => Some(Level::Trace),
            "debug" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "warn" => Some(Level::Warn),
            "error" => Some(Level::Error),
            "fatal" => Some(Level::Fatal),
            "off" => Some(Level::Off),
            _ => None,
        }
    }

    /// The level for a C ABI value, or `None` when the value is out of range.
    #[must_use]
    pub fn from_raw(raw: u32) -> Option<Level> {
        match raw {
            0 => Some(Level::Trace),
            1 => Some(Level::Debug),
            2 => Some(Level::Info),
            3 => Some(Level::Warn),
            4 => Some(Level::Error),
            5 => Some(Level::Fatal),
            6 => Some(Level::Off),
            _ => None,
        }
    }

    /// The upper-case name written in a line.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
            Level::Fatal => "FATAL",
            Level::Off => "OFF",
        }
    }

    /// The ANSI colour of the level field on a terminal.
    pub(crate) fn colour(self) -> &'static str {
        match self {
            Level::Trace => "\x1b[37m",
            Level::Debug => "\x1b[36m",
            Level::Info => "\x1b[32m",
            Level::Warn => "\x1b[33m",
            Level::Error => "\x1b[31m",
            Level::Fatal => "\x1b[1;31m",
            Level::Off => "\x1b[0m",
        }
    }
}
