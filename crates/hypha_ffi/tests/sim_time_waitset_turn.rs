//! Simulated time and the next `WaitSet` turn.

mod common;
use crate::common::attach;
use crate::common::create_node as create_test_node;
use crate::common::unique;

use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

static SIM_TIMER_COUNT: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_sim_timer(_missed: u64, _context: *mut std::ffi::c_void) -> HyphaCallbackResult {
    SIM_TIMER_COUNT.fetch_add(1, Ordering::Relaxed);
    HyphaCallbackResult { action: HYPHA_ACTION_STOP, status: 0 }
}

/// A clock-driven timer follows a simulated time jump; the next `WaitSet` turn fires it.
#[test]
fn test_simulation_time_advances_timers_on_the_next_waitset_turn() {
    let node = create_test_node(&unique("sim_time"));
    assert!(!node.is_null());

    assert_eq!(hypha_clock_enable_sim_time(0), HyphaStatus::Ok as i32);

    assert_eq!(hypha_clock_set_sim_time(1_000_000_000), HyphaStatus::Ok as i32);
    assert_eq!(hypha_clock_now_nanos(), 1_000_000_000);

    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);

    let mut timer_id: u64 = 0;
    assert_eq!(
        unsafe {
            hypha_waitset_attach(
                waitset,
                &attach::timer(100_000_000, Some(on_sim_timer), ptr::null_mut(), None),
                &raw mut timer_id,
            )
        },
        HyphaStatus::Ok
    );

    SIM_TIMER_COUNT.store(0, Ordering::Release);

    // The jump itself dispatches nothing.
    assert_eq!(hypha_clock_set_sim_time(1_350_000_000), HyphaStatus::Ok as i32);
    assert_eq!(SIM_TIMER_COUNT.load(Ordering::Acquire), 0);

    // The next WaitSet turn finds the timer already due and fires it.
    let mut report =
        HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, 0, &raw mut report) }, HyphaStatus::Ok);

    let count = SIM_TIMER_COUNT.load(Ordering::Acquire);
    assert!(
        count >= 1,
        "the timer must fire once the simulated clock passes its due time, got {count}"
    );

    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
    unsafe { hypha_node_destroy(node) };
}
