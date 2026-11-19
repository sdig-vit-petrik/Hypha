//! Safe, backend-independent state and orchestration used by the C ABI.

/// Acknowledgement rules of a lockstep `/clock`: the player and the clock driver.
pub mod clock_barrier;
mod execution;
mod executor;
pub mod port_limits;
pub mod process_restart;
pub mod replay_progress;
pub mod replay_steps;
mod schema;
/// The coordinate-frame buffer of a cycle's TF input.
pub mod tf_buffer;
pub mod timer_schedule;
/// Scheduling rules of data-triggered cycles.
pub mod trigger_schedule;
pub mod upstream_hold;
pub mod upstream_reports;

pub use execution::{ExecutionCounters, ExecutionSnapshot};
pub use executor::{RunDriver, RunError, RunState};
pub use schema::{ResolvedIdentity, SchemaFailure, SchemaHealth, SchemaState, SchemaTransport};
