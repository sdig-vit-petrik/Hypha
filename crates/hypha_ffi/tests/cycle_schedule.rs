//! Cycle schedule observations come from the timer that actually fires.
#![allow(clippy::expect_used)]

use std::ffi::{CString, c_void};
use std::ptr;
use std::sync::Mutex;

mod common;
use crate::common::create_node;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

static SERIAL: Mutex<()> = Mutex::new(());

struct Session {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    cycle: *mut HyphaCycle,
    times: Box<Mutex<Vec<HyphaCycleTime>>>,
}

extern "C" fn record_time(
    _: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let times = unsafe { &*context.cast::<Mutex<Vec<HyphaCycleTime>>>() };
    if let Ok(mut times) = times.lock() {
        times.push(unsafe { *time });
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

impl Session {
    fn new() -> Self {
        let text = format!("cycle_schedule_{}", std::process::id());
        let node = create_node(&text);
        assert_eq!(hypha_clock_enable_sim_time(0), HyphaStatus::Ok as i32);
        assert_eq!(hypha_clock_set_sim_time(1_000_000_000), HyphaStatus::Ok as i32);
        Self::with_node(node, &text)
    }

    fn with_node(node: *mut HyphaNode, text: &str) -> Self {
        let name = CString::new(text).expect("name");
        let mut waitset = ptr::null_mut();
        let mut cycle = ptr::null_mut();
        let times = Box::new(Mutex::new(Vec::new()));
        unsafe {
            assert_eq!(hypha_node_default_waitset(node, &raw mut waitset), HyphaStatus::Ok);
            assert_eq!(
                hypha_cycle_create(
                    waitset,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    50_000_000,
                    Some(record_time),
                    ptr::from_ref(times.as_ref()).cast_mut().cast(),
                    None,
                    &raw mut cycle
                ),
                HyphaStatus::Ok
            );
        }
        Self { node, waitset, cycle, times }
    }

    fn observe(&self, nanos: i64) {
        let mut report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        unsafe {
            assert_eq!(hypha_clock_set_sim_time(nanos), HyphaStatus::Ok as i32);
            assert_eq!(
                hypha_waitset_run(self.waitset, 2_000_000, &raw mut report),
                HyphaStatus::Ok
            );
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            assert_eq!(hypha_cycle_destroy(self.cycle), HyphaStatus::Ok);
            assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
            hypha_node_destroy(self.node);
        }
    }
}

#[test]
fn cycle_deadlines_follow_actual_timer_observations() {
    let _serial = SERIAL.lock().expect("serial");
    let session = Session::new();
    // Setup may take arbitrarily long; the actual timer starts at commit.
    unsafe {
        assert_eq!(hypha_clock_set_sim_time(2_000_000_000), HyphaStatus::Ok as i32);
        assert_eq!(hypha_cycle_commit(session.cycle), HyphaStatus::Ok);
    }
    // The deadlines sit on the 50 ms grid, the first one after the commit: 2.05
    // has expired at 2.075. It runs as its own step, with no missed period.
    session.observe(2_075_000_000);
    let first = session.times.lock().expect("times").first().copied().expect("first callback");
    assert_eq!(first.scheduled_nanos, 2_050_000_000);
    assert_eq!(first.missed_periods, 0);

    // 2.10 expires at 2.11: one step, with an elapsed time since the first.
    session.observe(2_110_000_000);
    let second = session.times.lock().expect("times").last().copied().expect("second callback");
    assert_eq!(second.scheduled_nanos, 2_100_000_000);
    assert_eq!(second.has_elapsed, 1);
    assert_eq!(second.missed_periods, 0);

    // 2.15, 2.20 and 2.25 have expired at 2.27: three steps, none of them missed.
    session.observe(2_270_000_000);
    let steps: Vec<HyphaCycleTime> = session.times.lock().expect("times").iter().copied().collect();
    let third = steps.last().copied().expect("third callback");
    let stepped: Vec<i64> = steps.iter().rev().take(3).rev().map(|t| t.scheduled_nanos).collect();
    assert_eq!(stepped, [2_150_000_000, 2_200_000_000, 2_250_000_000]);
    assert_eq!(third.has_elapsed, 1);
    assert_eq!(third.missed_periods, 0);
}

#[test]
fn waitset_destroy_releases_its_live_cycles() {
    let _serial = SERIAL.lock().expect("serial");
    let session = Session::new();
    assert_eq!(unsafe { hypha_cycle_commit(session.cycle) }, HyphaStatus::Ok);
    // The WaitSet owns the cycle, so only the node is left to release.
    let node = session.node;
    std::mem::forget(session);
    unsafe {
        assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
        hypha_node_destroy(node);
    }
}
