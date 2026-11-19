//! The operating-system calls of hypha that need `unsafe`, each behind a safe function.
//!
//! - [`signal`]: the SIGINT and SIGTERM handlers that stop the process's nodes, and the
//!   SIGBUS handler that names a full `/dev/shm`.
//! - [`map_read_only`]: a read-only memory map of a file, for a recording larger than memory.
//! - [`set_parent_death_signal`]: a child that gets a signal when its parent dies.
//!
//! Only `hypha_node` depends on this crate. Each `unsafe` block does one operation.

mod map;
#[cfg(target_os = "linux")]
mod process;
pub mod signal;

pub use map::{ReadOnlyMap, map_read_only};
#[cfg(target_os = "linux")]
pub use process::set_parent_death_signal;
