//! Framework time that moves backwards, through the C ABI. Every case holds [`SERIAL`].

mod common;
use crate::common::attach;
use crate::common::unique;

use std::ffi::{CString, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

static SERIAL: Mutex<()> = Mutex::new(());

const PERIOD_NANOS: i64 = 100_000_000;
/// How long each bounded run lasts, in real time: long enough for one timer wake.
const RUN_NANOS: u64 = 150_000_000;

/// A node on simulated time, released with the monotonic clock restored.
struct SimulatedNode {
    node: *mut HyphaNode,
    _serial: MutexGuard<'static, ()>,
}

impl SimulatedNode {
    fn new(label: &str, start_nanos: i64) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let name = CString::new(unique(label)).unwrap_or_default();
        let mut node = ptr::null_mut();
        let status = unsafe {
            hypha_node_create(
                name.as_ptr(),
                name.as_bytes().len(),
                ptr::null(),
                0,
                ptr::null(),
                &raw mut node,
            )
        };
        assert_eq!(status, HyphaStatus::Ok);
        let status = hypha_clock_enable_sim_time(0);
        assert_eq!(status, HyphaStatus::Ok as i32);
        let simulated = Self { node, _serial: serial };
        assert_eq!(hypha_clock_set_sim_time(start_nanos), HyphaStatus::Ok as i32);
        simulated
    }
}

impl Drop for SimulatedNode {
    fn drop(&mut self) {
        assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
        unsafe { hypha_node_destroy(self.node) };
    }
}

extern "C" fn count_fire(_missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    let fired = unsafe { &*context.cast::<AtomicU32>() };
    fired.fetch_add(1, Ordering::Relaxed);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn run_once(waitset: *mut HyphaWaitSet) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    let status = unsafe { hypha_waitset_run(waitset, RUN_NANOS, &raw mut report) };
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(report.status, 0, "the run failed: {}", report.status);
}

#[test]
fn a_waitset_timer_ends_the_run_on_a_rewind() {
    let start_nanos: i64 = 1_000_000_000_000;
    let simulated = SimulatedNode::new("timer", start_nanos);
    let mut waitset = ptr::null_mut();
    let status = unsafe { hypha_node_default_waitset(simulated.node, &raw mut waitset) };
    assert_eq!(status, HyphaStatus::Ok);
    let fired = AtomicU32::new(0);
    let context = ptr::from_ref(&fired).cast_mut().cast::<c_void>();
    let mut attachment: HyphaAttachmentId = 0;
    let status = unsafe {
        hypha_waitset_attach(
            waitset,
            &attach::timer(PERIOD_NANOS.unsigned_abs(), Some(count_fire), context, None),
            &raw mut attachment,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);

    assert_eq!(hypha_clock_set_sim_time(start_nanos + 350_000_000), HyphaStatus::Ok as i32);
    run_once(waitset);
    assert_eq!(fired.swap(0, Ordering::Relaxed), 1, "the timer follows a forward jump");

    let replay_start: i64 = 5_000_000_000;
    assert_eq!(hypha_clock_set_sim_time(replay_start), HyphaStatus::Ok as i32);
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    let status = unsafe { hypha_waitset_run(waitset, RUN_NANOS, &raw mut report) };
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(report.status, HyphaStatus::ClockRewound as i32, "nothing rewinds in place");
    assert_eq!(report.attachment, attachment, "the timer that saw it is named");
    assert_eq!(fired.swap(0, Ordering::Relaxed), 0, "the rewind fires nothing");
}

#[test]
fn a_waitset_timer_refuses_a_period_it_cannot_keep() {
    let simulated = SimulatedNode::new("timer_refusal", 1_000_000_000);
    let mut waitset = ptr::null_mut();
    let status = unsafe { hypha_node_default_waitset(simulated.node, &raw mut waitset) };
    assert_eq!(status, HyphaStatus::Ok);
    let too_long = u64::try_from(i64::MAX).unwrap_or(u64::MAX).saturating_add(1);

    for period_nanos in [0, too_long] {
        let mut attachment: HyphaAttachmentId = 7;
        let status = unsafe {
            hypha_waitset_attach(
                waitset,
                &attach::timer(period_nanos, Some(count_fire), ptr::null_mut(), None),
                &raw mut attachment,
            )
        };
        assert_eq!(status, HyphaStatus::InvalidArgument, "period {period_nanos}");
        assert_eq!(attachment, 0);
    }
}
