//! A cycle that drives `/clock` for a simulator: its settings at the C ABI.

use super::state::CycleId;
use super::types::HyphaClockDriverOptions;
use crate::status::Error;
use crate::waitset::HyphaWaitSet;
use hypha_runtime::clock_barrier::check_clock_value;

/// The driver settings of an uncommitted cycle.
#[derive(Debug, Clone)]
pub(crate) struct DriverSetup {
    pub(crate) options: HyphaClockDriverOptions,
    pub(crate) expected: Vec<String>,
}

/// Makes the uncommitted rate cycle `cycle` of `set` drive `/clock` with `options`.
///
/// # Errors
/// `InvalidArgument` for a cycle `set` does not hold, a start `/clock` cannot carry (negative,
/// or 2^32 seconds or more: the reason is then the thread's `hypha_last_error`), a zero
/// timeout, a negative or non-finite factor, a triggered cycle; `InvalidState` for a committed
/// cycle or a running `WaitSet`.
pub fn cycle_set_clock_driver(
    set: &HyphaWaitSet,
    cycle: CycleId,
    options: HyphaClockDriverOptions,
) -> Result<(), Error> {
    let cycle = set.cycle(cycle)?;
    let mut registration = cycle.configurable(set)?;
    let factor = options.real_time_factor;
    if let Err(reason) = check_clock_value(options.start_nanos) {
        return Err(crate::last_error::refuse(Error::InvalidArgument, &reason));
    }
    let refused = options.timeout_nanos == 0
        || !factor.is_finite()
        || factor < 0.0
        || cycle.period_nanos() == 0
        || !registration.runtime.triggers().is_empty();
    if refused {
        return Err(Error::InvalidArgument);
    }
    let expected = registration.driver.take().map_or_default(|setup| setup.expected);
    registration.driver = Some(DriverSetup { options, expected });
    Ok(())
}

/// Adds `name` to the participants the clock driver of the cycle `cycle` of `set` waits for.
///
/// # Errors
/// `InvalidArgument` for a cycle `set` does not hold, an empty name or a cycle that
/// drives no clock; `InvalidState`
/// for a committed cycle or a running `WaitSet`.
pub fn cycle_expect_participant(
    set: &HyphaWaitSet,
    cycle: CycleId,
    name: &str,
) -> Result<(), Error> {
    let cycle = set.cycle(cycle)?;
    let mut registration = cycle.configurable(set)?;
    if name.is_empty() {
        return Err(Error::InvalidArgument);
    }
    let setup = registration.driver.as_mut().ok_or(Error::InvalidArgument)?;
    if !setup.expected.iter().any(|known| known == name) {
        setup.expected.push(name.to_owned());
    }
    Ok(())
}
