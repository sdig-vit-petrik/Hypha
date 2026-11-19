//! The `/clock_progress` input of a node on simulated time. Every case holds [`SERIAL`].
#![allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]

use std::sync::PoisonError;

use std::ffi::{CStr, c_void};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use hypha_core::message::EncodeInto as _;
use hypha_msgs::hypha_msgs::ReplayProgress;
use hypha_runtime::replay_progress::REPLAY_PROGRESS_DEFINITION;

mod common;
use crate::common::replay::*;
use crate::common::{c_text, create_node, unique};
use hypha_ffi::waitset::*;
use hypha_ffi::*;

/// A node on simulated time with its `WaitSet`, and a second node that publishes
/// `/clock_progress` reports for it.
struct Listener {
    name: String,
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    peer: *mut HyphaNode,
    progress: *mut HyphaPublisher,
}

impl Listener {
    fn new(label: &str) -> Self {
        let name = unique(label);
        let node = create_sim_node(&name);
        let waitset = unsafe { create_waitset(node) };
        let peer = create_node(&unique("progress_peer"));
        let progress = unsafe {
            open_publisher(
                peer,
                "/clock_progress",
                ReplayProgress::TYPE_NAME,
                ReplayProgress::SCHEMA_HASH,
                REPLAY_PROGRESS_DEFINITION,
            )
        };
        Self { name, node, waitset, peer, progress }
    }

    /// Send one report, as a node named `node` that reached `clock_nanos`.
    fn report(&self, node: &str, clock_nanos: u64) {
        let message =
            ReplayProgress { node: node.to_owned(), clock_nanos, clock_sent_nanos: clock_nanos };
        let mut payload = vec![0_u8; message.encoded_len()];
        message.encode_into(&mut payload).expect("a report encodes");
        unsafe { send_payload(self.progress, &payload) };
    }

    /// Run the `WaitSet` for 5 ms and return the report, whatever its status.
    fn run(&self) -> HyphaWaitSetRunReport {
        let mut report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        let nanos = 5_000_000;
        assert_eq!(
            unsafe { hypha_waitset_run(self.waitset, nanos, &raw mut report) },
            HyphaStatus::Ok
        );
        report
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        unsafe {
            hypha_publisher_destroy(self.progress);
            hypha_node_destroy(self.node);
            hypha_node_destroy(self.peer);
        }
        assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
    }
}

#[test]
fn a_report_of_another_node_is_read_without_failing_the_run() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let listener = Listener::new("upstream_listener");
    listener.report("upstream_peer", 500);
    assert_eq!(listener.run().status, HyphaStatus::Ok as i32);
}

#[test]
fn a_report_of_this_node_is_skipped_without_failing_the_run() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let listener = Listener::new("own_report_listener");
    listener.report(&listener.name.clone(), 900);
    assert_eq!(listener.run().status, HyphaStatus::Ok as i32);
}

#[test]
fn a_report_that_does_not_decode_fails_the_run_with_corrupt_payload() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let listener = Listener::new("corrupt_report_listener");
    unsafe { send_payload(listener.progress, &[1, 2, 3]) };
    assert_eq!(listener.run().status, HyphaStatus::CorruptPayload as i32);
}

extern "C" fn idle_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn count_progress_publishers(info: *const HyphaServicePortInfo, user_data: *mut c_void) {
    let info = unsafe { &*info };
    let name = unsafe { CStr::from_ptr(info.name) };
    if name.to_bytes() == b"/clock_progress" {
        let counter = unsafe { &*user_data.cast::<AtomicUsize>() };
        counter.fetch_add(info.publishers, Ordering::SeqCst);
    }
}

fn progress_publishers(observer: *const HyphaNode) -> usize {
    let counter = AtomicUsize::new(0);
    let status = unsafe {
        hypha_list_service_ports(
            observer,
            Some(count_progress_publishers),
            (&raw const counter).cast_mut().cast(),
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    counter.load(Ordering::SeqCst)
}

#[test]
fn a_replay_cycle_makes_its_node_visible_on_the_progress_topic_before_any_clock() {
    let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let observer = create_node(&unique("early_observer"));
    assert_eq!(progress_publishers(observer), 0, "no node publishes progress yet");

    let node = create_sim_node(&unique("early_progress"));
    let waitset = unsafe { create_waitset(node) };
    let label = c_text("early_cycle");
    let mut cycle = ptr::null_mut();
    let status = unsafe {
        hypha_cycle_create(
            waitset,
            label.as_ptr(),
            label.as_bytes().len(),
            10_000_000,
            Some(idle_step),
            ptr::null_mut(),
            None,
            &raw mut cycle,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    // No `/clock` sample has been published, so the node has not reported anything. Its
    // progress publisher is open already, so a downstream sees the node wait for it.
    assert_eq!(progress_publishers(observer), 1, "the committed replay node is visible");

    unsafe {
        hypha_cycle_destroy(cycle);
        hypha_node_destroy(node);
        hypha_node_destroy(observer);
    }
}
