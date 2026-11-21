//! SIGINT and SIGTERM ask every node of the process to stop. Each test runs in its own process.
#![allow(clippy::expect_used)]

use std::sync::{Mutex, PoisonError};

mod common;
use crate::common::{create_node, unique};

use hypha_ffi::{HyphaNode, hypha_node_destroy, hypha_node_ok};

static SERIAL: Mutex<()> = Mutex::new(());

fn raise(signal: libc::c_int) {
    assert_eq!(unsafe { libc::raise(signal) }, 0);
}

struct Node(*mut HyphaNode);

impl Node {
    fn new(name: &str) -> Self {
        Self(create_node(name))
    }

    fn ok(&self) -> bool {
        unsafe { hypha_node_ok(self.0) != 0 }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        unsafe { hypha_node_destroy(self.0) };
    }
}

#[test]
fn sigterm_stops_the_node() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = Node::new(&unique("stop_signal"));
    assert!(node.ok());
    raise(libc::SIGTERM);
    assert!(!node.ok());
}

#[test]
fn sigint_stops_the_node() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = Node::new(&unique("stop_signal"));
    raise(libc::SIGINT);
    assert!(!node.ok());
}

#[test]
fn a_node_created_after_a_stop_signal_stops_at_once() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let stopped = Node::new(&unique("stop_signal"));
    raise(libc::SIGTERM);
    assert!(!stopped.ok());
    drop(stopped);

    // The process is ending: a node created after the signal does not run again.
    let next = Node::new(&unique("stop_signal"));
    assert!(!next.ok());
    let mut waitset = std::ptr::null_mut();
    let status = unsafe { hypha_ffi::hypha_node_default_waitset(next.0, &raw mut waitset) };
    assert_eq!(status, hypha_ffi::HyphaStatus::Ok);
    let mut report = hypha_ffi::waitset::HyphaWaitSetRunReport {
        reason: 99,
        status: 0,
        attachment: 0,
        slow_callbacks: 0,
    };
    // A 60 s limit: a run that ends as stopped, not at its deadline, stopped at once.
    let run = unsafe { hypha_ffi::hypha_waitset_run(waitset, 60_000_000_000, &raw mut report) };
    assert_eq!(run, hypha_ffi::HyphaStatus::Ok);
    assert_eq!(report.reason, hypha_ffi::waitset::HYPHA_RUN_REASON_STOPPED);
}
