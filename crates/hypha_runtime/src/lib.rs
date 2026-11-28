//! Safe, backend-independent state and orchestration used below the C ABI.
//!
//! # Module map
//! - `schema`: process-wide schema identity and definition cache
//! - `port_limits`: input supervision requirement checks (`silence_ms`, `unsupervised`, `advisory`)
//! - `tf_buffer`: coordinate-frame transform buffer, interpolation, and refusal tracking
//! - `timer_schedule` / `trigger_schedule`: deadline grids and triggered-cycle scheduling
//! - `clock_barrier` / `replay_progress` / `replay_steps` / `upstream_hold` / `upstream_reports`: lockstep `/clock` replay rules
//! - `process_restart`: `HYPHA_REJOIN_AFTER_NANOS` rewind-restart coordination
//! - `execution` / `executor`: execution counters, snapshots, and run-state helpers

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
