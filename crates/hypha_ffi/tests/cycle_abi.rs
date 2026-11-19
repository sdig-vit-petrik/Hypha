//! C ABI tests for input sources and cycles.
#![allow(clippy::expect_used, clippy::arithmetic_side_effects, clippy::too_many_lines)]

mod common;
use crate::common::c_text;
use crate::common::create_node;
use crate::common::emit_message;
use crate::common::unique;

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

static SERIAL: Mutex<()> = Mutex::new(());

unsafe fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    waitset
}

unsafe fn create_pubsub(
    node: *mut HyphaNode,
    topic_str: &str,
) -> (*mut HyphaPublisher, *mut HyphaSubscriber) {
    unsafe { create_pubsub_carrying(node, topic_str, 0) }
}

/// A notified publisher and subscriber on `topic_str`, the publisher carrying
/// messages of up to `max_payload_dynamic` bytes (zero: growable).
unsafe fn create_pubsub_carrying(
    node: *mut HyphaNode,
    topic_str: &str,
    max_payload_dynamic: u64,
) -> (*mut HyphaPublisher, *mut HyphaSubscriber) {
    let topic = c_text(topic_str);
    let data_type = c_text("std_msgs/Counter");
    let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
    let definition = c_text("");

    let qos = hypha_ffi::HyphaQos { max_payload_dynamic, ..hypha_ffi::HyphaQos::default() };
    let pub_qos = HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos };
    let mut publisher = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publish_with_qos(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                data_type.as_ptr(),
                data_type.as_bytes().len(),
                md5.as_ptr(),
                md5.as_bytes().len(),
                definition.as_ptr(),
                0,
                &raw const pub_qos,
                &raw mut publisher,
            )
        },
        HyphaStatus::Ok
    );

    let sub_qos = HyphaSubscriberOptions {
        depth: 4,
        no_publisher_warning_after_nanos: 0,
        qos: hypha_ffi::HyphaQos::default(),
    };
    let mut subscriber = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_subscribe_typed(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                std::ptr::null(),
                0,
                md5.as_ptr(),
                md5.as_bytes().len(),
                std::ptr::null(),
                0,
                &raw const sub_qos,
                &raw mut subscriber,
            )
        },
        HyphaStatus::Ok
    );

    (publisher, subscriber)
}

unsafe fn send_sample(publisher: *mut HyphaPublisher, data: &[u8]) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(publisher, data.len(), &raw mut loan, &raw mut bytes) },
        HyphaStatus::Ok
    );
    unsafe { ptr::copy_nonoverlapping(data.as_ptr(), bytes, data.len()) };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
}

struct SimulatedNode {
    node: *mut HyphaNode,
    _serial: MutexGuard<'static, ()>,
}

impl SimulatedNode {
    fn new(label: &str, start_nanos: i64) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let node = create_node(&unique(label));
        assert_eq!(hypha_clock_enable_sim_time(0), HyphaStatus::Ok as i32);
        let simulated = Self { node, _serial: serial };
        set_sim_clock(start_nanos);
        simulated
    }
}

fn set_sim_clock(nanos: i64) {
    assert_eq!(hypha_clock_set_sim_time(nanos), HyphaStatus::Ok as i32);
}

impl Drop for SimulatedNode {
    fn drop(&mut self) {
        unsafe {
            assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
            hypha_node_destroy(self.node);
        }
    }
}
extern "C" fn empty_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[path = "cycle_abi/emission.rs"]
mod emission;
#[path = "cycle_abi/execution.rs"]
mod execution;
#[path = "cycle_abi/held.rs"]
mod held;
#[path = "cycle_abi/registration.rs"]
mod registration;
#[path = "cycle_abi/sources.rs"]
mod sources;
#[path = "cycle_abi/trigger.rs"]
mod trigger;

fn run_waitset_for(waitset: *mut HyphaWaitSet, nanos: u64) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    let _ = unsafe { hypha_waitset_run(waitset, nanos, &raw mut report) };
}

/// Run `waitset` in 10 ms slices until `done` holds. Returns false when the
/// bound passes first.
fn run_until(
    waitset: *mut HyphaWaitSet,
    bound: std::time::Duration,
    mut done: impl FnMut() -> bool,
) -> bool {
    let started = std::time::Instant::now();
    while !done() {
        if started.elapsed() >= bound {
            return false;
        }
        run_waitset_for(waitset, 10_000_000);
    }
    true
}

const POLL_BOUND: std::time::Duration = std::time::Duration::from_secs(10);
