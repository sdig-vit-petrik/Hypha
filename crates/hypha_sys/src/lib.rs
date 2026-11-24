//! The operating-system calls of hypha, each behind a safe function; every `unsafe` one is here.
//!
//! - [`signal`]: the SIGINT and SIGTERM handlers that stop the process's nodes, and the
//!   SIGBUS handler that names a full `/dev/shm`.
//! - [`map_read_only`]: a read-only memory map of a file, for a recording larger than memory.
//! - [`set_parent_death_signal`]: a child that gets a signal when its parent dies.
//! - [`process_lives`]: whether a process exists, for the run a test domain names.
//!
//! Only `hypha_node` depends on this crate. Each `unsafe` block does one operation.

mod liveness;
mod map;
#[cfg(target_os = "linux")]
mod process;
pub mod signal;

pub use liveness::process_lives;
pub use map::{ReadOnlyMap, map_read_only};
#[cfg(target_os = "linux")]
pub use process::set_parent_death_signal;
