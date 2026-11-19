//! The framework clock follows the host clock, and simulated time when enabled.

use hypha_node::sdk::clock::{disable_sim_time, enable_sim_time, now_nanos, steady_now_nanos};

/// 2026-10-08T12:00:00Z, far from the host clock of any test run.
const SIM_START_NANOS: i64 = 1_791_460_800_000_000_000;

#[test]
fn the_clock_reads_the_host_clock_until_simulated_time_is_enabled() {
    let host_before = now_nanos();
    assert!(host_before > 0, "the framework clock reads the host's UNIX time");
    let steady_first = steady_now_nanos();
    let steady_second = steady_now_nanos();
    assert!(steady_second >= steady_first, "the steady clock never goes back");

    assert_eq!(enable_sim_time(SIM_START_NANOS), Ok(()));
    assert_eq!(now_nanos(), SIM_START_NANOS, "simulated time stands at its start");
    assert!(steady_now_nanos() >= steady_second, "simulated time leaves the steady clock alone");

    assert_eq!(disable_sim_time(), Ok(()));
    assert!(now_nanos() >= host_before, "the framework clock reads the host clock again");
}
