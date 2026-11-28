//! The logging engine of hypha.
//!
//! One process has one logging configuration; the default is console on, level `debug`,
//! no file sinks. [`prepare`] validates the `[log]` table and [`commit`] applies it. The
//! engine creates no thread: one `write` call per target on the thread that logs.
//! Rust code logs through the `log` crate facade, installed by [`install`].
//!
//! ```
//! use hypha_log::{enabled, register_module, report, Level};
//!
//! let module = register_module("hypha/example")?;
//! assert!(enabled(module, Level::Info));
//! assert!(enabled(module, Level::Debug));
//! assert!(!enabled(module, Level::Trace));
//! report("hypha/example", Level::Trace, format_args!("not formatted: {}", 1));
//! # Ok::<(), hypha_log::LogError>(())
//! ```

mod config;
mod console;
mod engine;
mod facade;
mod file;
mod level;
mod render;

use std::fmt;

pub use config::slow_callback_ms;
pub use engine::{PreparedConfig, commit, prepare, validate};
pub use facade::install;
pub use level::Level;

/// The identifier of a registered module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleId(pub u32);

/// A refusal by the engine.
#[derive(Debug, PartialEq, Eq)]
pub enum LogError {
    /// An invalid value. The message names the file, the table and the key.
    InvalidArgument(String),
    /// A file that another holder locked, or a conflicting process configuration.
    InvalidState(String),
    /// The engine already has 256 modules.
    ResourceLimit,
}

impl fmt::Display for LogError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogError::InvalidArgument(message) | LogError::InvalidState(message) => {
                out.write_str(message)
            }
            LogError::ResourceLimit => out.write_str("the engine has 256 modules"),
        }
    }
}

impl std::error::Error for LogError {}

/// Counts of faults that a caller cannot act on. Every counter wraps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Health {
    /// Lines that reached at least one target.
    pub lines_written: u64,
    /// Target writes that failed, failed rolls included.
    pub write_failures: u64,
    /// Lines with an invalid module or level, and lines the ABI boundary refused.
    pub rejected_lines: u64,
    /// Console lines dropped because standard error was not read. Each also counts in
    /// `write_failures`.
    pub console_lines_dropped: u64,
}

/// Register a module. The same name gives the same identifier. The name has
/// 1 to 64 bytes from `A-Z`, `a-z`, `0-9`, `_` and `/`. The module `default`
/// exists at start and counts toward the limit of 256.
///
/// # Errors
///
/// `InvalidArgument` for an invalid name, and `ResourceLimit` for a new name
/// when 256 modules exist.
pub fn register_module(name: &str) -> Result<ModuleId, LogError> {
    engine::engine().register(name)
}

/// The module that lines of `name` are written under: `name` itself, or `default` once
/// 256 modules exist and `name` is not one of them.
///
/// # Errors
///
/// `InvalidArgument` for an invalid name.
pub fn module_or_default(name: &str) -> Result<ModuleId, LogError> {
    match register_module(name) {
        Err(LogError::ResourceLimit) => Ok(ModuleId(0)),
        other => other,
    }
}

/// True when a line at `level` reaches at least one target. No lock.
#[must_use]
pub fn enabled(module: ModuleId, level: Level) -> bool {
    engine::engine().enabled(module, level)
}

/// Write one entry, preserving line breaks. An invalid module or level counts as a
/// rejected line, and a disk fault counts in [`health`].
pub fn write(module: ModuleId, level: Level, text: &[u8]) {
    engine::engine().write(module, level, text);
}

/// Count a line that the ABI boundary refused, for example a null message.
pub fn count_rejected_line() {
    engine::engine().count_rejected();
}

/// The fault counters of the process.
#[must_use]
pub fn health() -> Health {
    engine::engine().health()
}

/// A diagnostic of the framework. Registers `module_name` on first use.
pub fn report(module_name: &str, level: Level, message: fmt::Arguments<'_>) {
    let Ok(module) = module_or_default(module_name) else {
        count_rejected_line();
        return;
    };
    if !enabled(module, level) {
        return;
    }
    write(module, level, message.to_string().as_bytes());
}
