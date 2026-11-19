//! The process-wide choice of clock: simulated when enabled, else UTC wall time.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{PoisonError, RwLock, RwLockReadGuard};

use crate::{Duration, Time};

use crate::sources::{SimulatedClock, steady_now, wall_now};

/// The simulated clock, or `None` while framework time is the UTC wall clock.
static SIMULATED: RwLock<Option<SimulatedClock>> = RwLock::new(None);

/// True while `SIMULATED` holds a clock, so the wall clock path takes no lock.
static SIM_ENABLED: AtomicBool = AtomicBool::new(false);

/// A poisoned lock still holds a valid clock: a panic elsewhere must not stop the clock.
fn simulated() -> RwLockReadGuard<'static, Option<SimulatedClock>> {
    SIMULATED.read().unwrap_or_else(PoisonError::into_inner)
}

/// Start simulated framework time reading `start`.
///
/// [`sim_time_has_value`] is true afterwards. Replaces an enabled simulated clock.
/// Returns `false`, and changes nothing, when `start` is negative.
#[must_use]
pub fn enable_sim_time(start: Time) -> bool {
    enable(start, true)
}

/// Start simulated framework time reading `start`, a placeholder that is not a
/// clock value yet: [`sim_time_has_value`] is false until the first set or advance.
///
/// Returns `false`, and changes nothing, when `start` is negative.
#[must_use]
pub fn enable_sim_time_placeholder(start: Time) -> bool {
    enable(start, false)
}

fn enable(start: Time, has_value: bool) -> bool {
    if start.nanos() < 0 {
        return false;
    }
    install(Some(SimulatedClock::starting_at(start, has_value)));
    true
}

/// Replace the simulated clock; the flag is stored while the write lock is held.
fn install(clock: Option<SimulatedClock>) {
    let enabled = clock.is_some();
    let mut slot = SIMULATED.write().unwrap_or_else(PoisonError::into_inner);
    *slot = clock;
    SIM_ENABLED.store(enabled, Ordering::Release);
}

/// Apply `change` to the simulated clock. False when simulated time is disabled.
fn with_simulated(change: impl FnOnce(&SimulatedClock)) -> bool {
    let slot = simulated();
    let Some(clock) = slot.as_ref() else {
        return false;
    };
    change(clock);
    true
}

/// True when simulated time is enabled and its reading was chosen: enabled at a
/// value, or set or advanced since a placeholder enable.
#[must_use]
pub fn sim_time_has_value() -> bool {
    simulated().as_ref().is_some_and(SimulatedClock::has_value)
}

/// Return framework time to the UTC wall clock.
pub fn disable_sim_time() {
    install(None);
}

/// Set simulated framework time to `instant`.
///
/// Returns `false`, and changes nothing, while simulated time is disabled or `instant`
/// is negative.
#[must_use]
pub fn set_sim_time(instant: Time) -> bool {
    if instant.nanos() < 0 {
        return false;
    }
    with_simulated(|clock| clock.set(instant))
}

/// Move simulated framework time forward by `span`.
///
/// Returns `false`, and changes nothing, while simulated time is disabled or `span` is
/// negative.
#[must_use]
pub fn advance_sim_time(span: Duration) -> bool {
    if span.nanos() < 0 {
        return false;
    }
    with_simulated(|clock| clock.advance(span))
}

/// Framework time: simulated when enabled, else the UTC wall clock.
///
/// The time of stamps, logs and recordings. Scheduling reads [`scheduling_now`];
/// transport timeouts and frame age read [`crate::steady_now`].
#[must_use]
pub fn now() -> Time {
    if !is_sim_time() {
        return wall_now();
    }
    simulated().as_ref().map_or_else(wall_now, SimulatedClock::now)
}

/// The reading that schedules work: simulated time when enabled, else the monotonic
/// host clock ([`crate::steady_now`]). Compare host readings only with each other,
/// never with a stamp.
#[must_use]
pub fn scheduling_now() -> Time {
    if is_sim_time() { now() } else { steady_now() }
}

/// True while framework time is simulated. An atomic read, no lock.
#[must_use]
pub fn is_sim_time() -> bool {
    SIM_ENABLED.load(Ordering::Acquire)
}
