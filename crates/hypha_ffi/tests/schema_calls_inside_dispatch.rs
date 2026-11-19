//! Schema calls from a dispatcher callback: one that waits for a round trip is refused
//! with `InvalidState`; `hypha_schema_lookup` of a definition the node holds is not.
#![allow(clippy::expect_used)]

mod common;
use crate::common::attach;
use crate::common::create_node;
use crate::common::unique;

use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

const DATA_TYPE: &str = "std_msgs/String";
const MD5: &str = "992ce8a1687cec8c8bd883ec73ca41d1";
const MISSING_MD5: &str = "11111111111111111111111111111111";
const DEFINITION: &str = "string data\n";
const MILLISECOND: u64 = 1_000_000;

/// What the timer callback saw. Each field is one call made inside the run.
#[derive(Debug, Clone, Copy)]
struct Observed {
    /// `hypha_schema_lookup` for a definition the node holds.
    held: HyphaStatus,
    held_len: usize,
    /// `hypha_schema_lookup` for a definition nobody serves.
    unheld: HyphaStatus,
}

/// The node, the topic and the slot the timer callback writes to.
struct Probe {
    node: *mut HyphaNode,
    topic: String,
    observed: Mutex<Option<Observed>>,
}

unsafe fn advertise(node: *mut HyphaNode, topic: &str) -> *mut HyphaPublisher {
    let mut publisher: *mut HyphaPublisher = ptr::null_mut();
    let status = unsafe {
        hypha_publish_with_qos(
            node,
            topic.as_ptr().cast(),
            topic.len(),
            DATA_TYPE.as_ptr().cast(),
            DATA_TYPE.len(),
            MD5.as_ptr().cast(),
            MD5.len(),
            DEFINITION.as_ptr().cast(),
            DEFINITION.len(),
            &HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() },
            &raw mut publisher,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    publisher
}

fn lookup(node: *mut HyphaNode, topic: &str, md5: &str) -> (HyphaStatus, usize) {
    let mut length = 0_usize;
    let status = unsafe {
        hypha_schema_lookup(
            node,
            topic.as_ptr().cast(),
            topic.len(),
            DATA_TYPE.as_ptr().cast(),
            DATA_TYPE.len(),
            md5.as_ptr().cast(),
            md5.len(),
            ptr::null_mut(),
            0,
            &raw mut length,
        )
    };
    (status, length)
}

impl Probe {
    fn observe(&self) -> Observed {
        let (held, held_len) = lookup(self.node, &self.topic, MD5);
        let (unheld, _) = lookup(self.node, &self.topic, MISSING_MD5);
        Observed { held, held_len, unheld }
    }
}

extern "C" fn probe_calls(_missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    let probe = unsafe { &*context.cast::<Probe>() };
    let observed = probe.observe();
    *probe.observed.lock().expect("the probe slot") = Some(observed);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn observe_inside_a_run(node: *mut HyphaNode, topic: &str) -> Observed {
    let probe = Probe { node, topic: topic.to_owned(), observed: Mutex::new(None) };
    let mut waitset = ptr::null_mut();
    let created = unsafe { hypha_node_default_waitset(node, &raw mut waitset) };
    assert_eq!(created, HyphaStatus::Ok);
    let mut timer = 0;
    let attached = unsafe {
        hypha_waitset_attach(
            waitset,
            &attach::timer(
                MILLISECOND,
                Some(probe_calls),
                ptr::from_ref(&probe).cast_mut().cast(),
                None,
            ),
            &raw mut timer,
        )
    };
    assert_eq!(attached, HyphaStatus::Ok);
    let mut report =
        HyphaWaitSetRunReport { reason: 9, status: 9, attachment: 9, slow_callbacks: 9 };
    let ran = unsafe { hypha_waitset_run(waitset, 50 * MILLISECOND, &raw mut report) };
    assert_eq!(ran, HyphaStatus::Ok);
    let observed = probe.observed.lock().expect("the probe slot").take();
    observed.expect("the timer callback ran inside the run")
}

#[test]
fn a_callback_reads_a_definition_the_node_already_holds() {
    let node = create_node(&unique("held_schema"));
    let topic = format!("/{}", unique("topic"));
    let publisher = unsafe { advertise(node, &topic) };
    let (status, length) = lookup(node, &topic, MD5);
    assert_eq!(status, HyphaStatus::Ok, "the definition resolves before the run");
    assert_eq!(length, DEFINITION.len());

    let observed = observe_inside_a_run(node, &topic);

    assert_eq!(observed.held, HyphaStatus::Ok, "a held definition needs no round trip");
    assert_eq!(observed.held_len, DEFINITION.len());
    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_callback_refuses_a_definition_that_needs_a_round_trip() {
    let node = create_node(&unique("unheld_schema"));
    let topic = format!("/{}", unique("topic"));
    let publisher = unsafe { advertise(node, &topic) };

    let observed = observe_inside_a_run(node, &topic);

    assert_eq!(
        observed.unheld,
        HyphaStatus::InvalidState,
        "an unheld definition would wait for a round trip, so the callback is refused"
    );
    unsafe { hypha_publisher_destroy(publisher) };
    unsafe { hypha_node_destroy(node) };
}
