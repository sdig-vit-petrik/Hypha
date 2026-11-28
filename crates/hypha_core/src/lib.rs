//! Shared framework data types and deterministic state.
//!
//! # Module map
//! - `port`: topic specs, reserved names (`/hypha/`), wildcards, `QoS` values, port errors
//! - `frame_validation`: frame header/CRC/sequence/timestamp validation rules
//! - `message`: message trait and schema identity abstractions
//! - `parameters` / `parameters_load` / `parameter_constraints`: TOML config loader, `${ENV}`, `include`, range checks
//! - `dispatch`: `WaitSet` dispatch limits and fairness budgets
//! - `timer`: polled timer cadence state
//! - `introspection`: node manifest and configuration introspection records

extern crate alloc;

pub mod dispatch;
pub mod frame_validation;
pub mod introspection;
pub mod message;
pub mod parameter_constraints;
pub mod parameters;
mod parameters_load;
pub mod port;
pub mod timer;

/// The guard of `mutex`, taken as it is when a holder panicked. A shipped build aborts
/// on a panic, so no lock is ever poisoned there: the recovery serves test builds,
/// which unwind (`docs/rust-style.md`, "Locks").
#[inline]
pub(crate) fn locked<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
