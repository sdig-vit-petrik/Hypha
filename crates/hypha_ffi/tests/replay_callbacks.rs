//! Plain callbacks take no part in the replay lockstep (#153): only cycles are held,
//! ordered and reported. A node with only plain callbacks is not on `/clock_progress`,
//! so the player does not wait for it and no downstream cycle holds a deadline for it.

use std::ffi::c_void;
use std::ptr;

mod common;
use crate::common::replay::{
    MILLISECOND, SECOND, SERIAL, create_sim_node, open_subscriber, run_for, set_clock,
};
use crate::common::{attach, unique};
use hypha_ffi::waitset::*;
use hypha_ffi::*;
use hypha_msgs::hypha_msgs::ReplayProgress;

extern "C" fn tick(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_node_with_only_plain_callbacks_is_not_on_the_progress_topic() {
    let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let observer = crate::common::create_node(&unique("observer"));
    // SAFETY: the observer is live, and the topic and hash are valid text.
    let reader =
        unsafe { open_subscriber(observer, "/clock_progress", ReplayProgress::SCHEMA_HASH, 8) };
    let publishers = || {
        let mut count = usize::MAX;
        // SAFETY: the reader is live, and `count` is one writable size_t.
        let status = unsafe { hypha_subscriber_publisher_count(reader, &raw mut count) };
        assert_eq!(status, HyphaStatus::Ok);
        count
    };
    let plain = create_sim_node(&unique("plain"));
    let mut waitset = ptr::null_mut();
    let record = attach::timer(100_000_000, Some(tick), ptr::null_mut(), None);
    let mut attachment = 0;
    // SAFETY: the node is live, each out pointer is valid, and the timer has no context.
    unsafe {
        assert_eq!(hypha_node_default_waitset(plain, &raw mut waitset), HyphaStatus::Ok);
        let status = hypha_waitset_attach(waitset, &raw const record, &raw mut attachment);
        assert_eq!(status, HyphaStatus::Ok);
    }
    // The plain timer runs through several clock values: still nothing reports.
    for step in 1..=5 {
        set_clock(SECOND + step * 100 * MILLISECOND);
        run_for(waitset, 1_000_000);
    }
    assert_eq!(publishers(), 0, "neither waited for nor an upstream of anyone");
    // SAFETY: each handle is destroyed once, the reader before its node.
    unsafe {
        hypha_subscriber_destroy(reader);
        hypha_node_destroy(plain);
        hypha_node_destroy(observer);
    }
    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
}

extern "C" fn count_missed(missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    // SAFETY: the test passes a `Mutex<Vec<u64>>` that outlives the node.
    let calls = unsafe { &*context.cast::<std::sync::Mutex<Vec<u64>>>() };
    calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(missed);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_plain_timer_starts_on_the_first_clock_value_not_on_the_placeholder() {
    let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let calls = Box::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let node = create_sim_node(&unique("placeholder"));
    let context = std::ptr::from_ref(&*calls).cast_mut().cast::<c_void>();
    let record = attach::timer(100_000_000, Some(count_missed), context, None);
    let mut waitset = ptr::null_mut();
    let mut attachment = 0;
    // SAFETY: the node is live, each out pointer is valid, and the context outlives it.
    unsafe {
        assert_eq!(hypha_node_default_waitset(node, &raw mut waitset), HyphaStatus::Ok);
        let status = hypha_waitset_attach(waitset, &raw const record, &raw mut attachment);
        assert_eq!(status, HyphaStatus::Ok);
    }
    // A turn on the placeholder zero, before any clock value, then the first value.
    run_for(waitset, 1_000_000);
    set_clock(1_000 * SECOND);
    run_for(waitset, 1_000_000);
    set_clock(1_000 * SECOND + 100 * MILLISECOND);
    run_for(waitset, 1_000_000);
    let seen = calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    assert_eq!(seen, [0], "one period after the first clock value, none missed");
    // SAFETY: the node is destroyed once, before its context.
    unsafe { hypha_node_destroy(node) };
    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
}
