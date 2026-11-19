//! Time and duration for hypha.
//!
//! The value types, the two operating-system clocks, the simulated clock, and the
//! process-wide registry that decides which of them [`now`] reads. All arithmetic
//! operators saturate at [`i64::MIN`] and [`i64::MAX`]. [`Cadence`] is the period
//! kernel of the polled timer in `hypha_core`.
//!
//! ```
//! use hypha_time::{Cadence, Duration, Time};
//!
//! let mut cadence = Cadence::new(Duration::from_millis(10), Time::from_nanos(0));
//! assert_eq!(cadence.advance(Time::from_nanos(25_000_000)).expired, 2);
//! ```
//!
//! # Process Clock
//!
//! ```
//! use hypha_time::{Duration, Time};
//!
//! hypha_time::enable_sim_time(Time::from_nanos(10_000_000_000));
//! assert!(hypha_time::advance_sim_time(Duration::from_secs(2)));
//! assert_eq!(hypha_time::now(), Time::from_nanos(12_000_000_000));
//! hypha_time::disable_sim_time();
//! ```

mod cadence;
mod duration;
mod format;
mod registry;
mod sources;
mod step_time;
mod time;
mod utc;

pub use cadence::{Advance, Cadence};
pub use duration::Duration;
pub use registry::{
    advance_sim_time, disable_sim_time, enable_sim_time, enable_sim_time_placeholder, is_sim_time,
    now, scheduling_now, set_sim_time, sim_time_has_value,
};
pub use sources::{steady_now, try_wall_now, wall_now};
pub use step_time::{StepTimeGuard, framework_now};
pub use time::Time;
pub use utc::Utc;

/// Nanoseconds in one second.
pub const NANOS_PER_SECOND: i64 = 1_000_000_000;
