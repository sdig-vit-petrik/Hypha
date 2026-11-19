//! The transform buffer of a plain node through the C ABI.
#![expect(clippy::expect_used, reason = "a failed test setup stops the test with its message")]
#![expect(clippy::float_cmp, reason = "the translations compared are exact binary sums")]

mod common;
use crate::common::replay::{SERIAL, create_waitset, run_for, run_status, send_payload};
use crate::common::{c_text, create_node, test_qos, unique};

use std::ffi::c_void;
use std::ptr;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hypha_core::message::EncodeInto as _;
use hypha_ffi::waitset::*;
use hypha_ffi::*;
use hypha_msgs::geometry_msgs::{Quaternion, Transform, TransformStamped, Vector3};
use hypha_msgs::std_msgs::Header;
use hypha_msgs::tf2_msgs::TFMessage;
use hypha_wire::ros_time::RosTime;

const SECOND: i64 = 1_000_000_000;
const CACHE_NANOS: u64 = 10_000_000_000;
const SETTLE_NANOS: u64 = 100_000_000;
const EMPTY: HyphaTransform =
    HyphaTransform { translation: [0.0; 3], rotation_xyzw: [0.0; 4], stamp_nanos: 0 };

/// A `TFMessage` with one transform of `child` in `parent` at `stamp_nanos`, by `xyz`.
fn tf_message(parent: &str, child: &str, stamp_nanos: i64, xyz: [f64; 3]) -> Vec<u8> {
    let nanos = u64::try_from(stamp_nanos).expect("a stamp after 1970");
    let secs = u32::try_from(nanos / 1_000_000_000).expect("a stamp before 2106");
    let subsec = u32::try_from(nanos % 1_000_000_000).expect("below one second");
    let message = TFMessage {
        transforms: vec![TransformStamped {
            header: Header { seq: 0, stamp: RosTime::new(secs, subsec), frame_id: parent.into() },
            child_frame_id: child.into(),
            transform: Transform {
                translation: Vector3 { x: xyz[0], y: xyz[1], z: xyz[2] },
                rotation: Quaternion { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
            },
        }],
    };
    let mut payload = vec![0_u8; message.encoded_len()];
    message.encode_into(&mut payload).expect("a TFMessage encodes");
    payload
}

/// A `tf2_msgs/TFMessage` publisher on `topic` with `qos`, latched when `latch`.
fn tf_publisher(
    node: *mut HyphaNode,
    topic: &str,
    latch: bool,
    qos: HyphaQos,
) -> *mut HyphaPublisher {
    let topic = c_text(topic);
    let data_type = c_text(TFMessage::TYPE_NAME);
    let md5 = c_text(TFMessage::SCHEMA_HASH);
    let options = HyphaPublisherOptions { latch: i32::from(latch), loaned_samples: 0, qos };
    let mut publisher = ptr::null_mut();
    // SAFETY: `node` is live; every text comes with its length; the outputs are locals.
    let status = unsafe {
        hypha_publish_with_qos(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            data_type.as_ptr(),
            data_type.as_bytes().len(),
            md5.as_ptr(),
            md5.as_bytes().len(),
            c"".as_ptr(),
            0,
            &raw const options,
            &raw mut publisher,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    publisher
}

fn create_buffer(
    node: *mut HyphaNode,
    tf: &str,
    tf_static: &str,
    qos: &HyphaQos,
    cache_nanos: u64,
) -> (HyphaStatus, *mut HyphaTransformBuffer) {
    // A sentinel: a refused create writes null here.
    let mut buffer = ptr::dangling_mut::<HyphaTransformBuffer>();
    // SAFETY: `node` is null or live; the topics come with their lengths; the rest are locals.
    let status = unsafe {
        hypha_transform_buffer_create(
            node,
            tf.as_ptr().cast(),
            tf.len(),
            tf_static.as_ptr().cast(),
            tf_static.len(),
            qos,
            cache_nanos,
            &raw mut buffer,
        )
    };
    (status, buffer)
}

/// One lookup: its status, result and transform. `None` asks for the latest time.
fn lookup(
    buffer: *const HyphaTransformBuffer,
    target: &str,
    source: &str,
    at: Option<i64>,
) -> (HyphaStatus, u32, HyphaTransform) {
    let mut result = u32::MAX;
    let mut out = EMPTY;
    // SAFETY: `buffer` is live; the names come with their lengths; the outputs are locals.
    let status = unsafe {
        hypha_transform_buffer_lookup(
            buffer,
            target.as_ptr().cast(),
            target.len(),
            source.as_ptr().cast(),
            source.len(),
            at.unwrap_or(0),
            u32::from(at.is_none()),
            &raw mut result,
            &raw mut out,
        )
    };
    (status, result, out)
}

fn result_of(
    buffer: *const HyphaTransformBuffer,
    target: &str,
    source: &str,
    at: Option<i64>,
) -> u32 {
    let (status, result, _) = lookup(buffer, target, source, at);
    assert_eq!(status, HyphaStatus::Ok);
    result
}

fn found(
    buffer: *const HyphaTransformBuffer,
    target: &str,
    source: &str,
    at: Option<i64>,
) -> HyphaTransform {
    let (status, result, transform) = lookup(buffer, target, source, at);
    assert_eq!((status, result), (HyphaStatus::Ok, HYPHA_TF_FOUND), "{target} <- {source}");
    transform
}

fn subscriber_count(publisher: *mut HyphaPublisher) -> usize {
    let mut count = usize::MAX;
    // SAFETY: `publisher` is live; `count` is a local.
    let status = unsafe { hypha_publisher_subscriber_count(publisher, &raw mut count) };
    assert_eq!(status, 1, "the count is exact");
    count
}

/// A node with a `/tf` and a latched `/tf_static` publisher on test topics, and a
/// transform buffer over both.
struct Rig {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    tf: *mut HyphaPublisher,
    tf_static: *mut HyphaPublisher,
    buffer: *mut HyphaTransformBuffer,
}

impl Rig {
    fn new(label: &str) -> Self {
        let node = create_node(&unique(label));
        let tf_topic = format!("/{}", unique("tf"));
        let static_topic = format!("/{}", unique("tf_static"));
        let tf = tf_publisher(node, &tf_topic, false, test_qos());
        let tf_static = tf_publisher(node, &static_topic, true, test_qos());
        let (status, buffer) =
            create_buffer(node, &tf_topic, &static_topic, &test_qos(), CACHE_NANOS);
        assert_eq!(status, HyphaStatus::Ok);
        // SAFETY: `node` is live.
        let waitset = unsafe { create_waitset(node) };
        Self { node, waitset, tf, tf_static, buffer }
    }

    fn send_dynamic(&self, parent: &str, child: &str, stamp_nanos: i64, xyz: [f64; 3]) {
        // SAFETY: the publisher is live.
        unsafe { send_payload(self.tf, &tf_message(parent, child, stamp_nanos, xyz)) };
    }

    fn send_static(&self, parent: &str, child: &str, xyz: [f64; 3]) {
        // SAFETY: the publisher is live.
        unsafe { send_payload(self.tf_static, &tf_message(parent, child, 0, xyz)) };
    }

    fn settle(&self) {
        run_for(self.waitset, SETTLE_NANOS);
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // SAFETY: every handle is live and released once, the buffer before its node.
        unsafe {
            assert_eq!(hypha_transform_buffer_destroy(self.buffer), HyphaStatus::Ok);
            hypha_publisher_destroy(self.tf);
            hypha_publisher_destroy(self.tf_static);
            hypha_node_destroy(self.node);
        }
    }
}

#[test]
fn a_lookup_between_two_dynamic_stamps_interpolates() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("tfb_interpolates");
    rig.send_dynamic("world", "base", SECOND, [0.0, 0.0, 0.0]);
    rig.send_dynamic("world", "base", 3 * SECOND, [2.0, 4.0, 0.0]);
    rig.settle();
    let transform = found(rig.buffer, "world", "base", Some(2 * SECOND));
    assert_eq!(transform.translation, [1.0, 2.0, 0.0]);
    assert_eq!(transform.rotation_xyzw, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(transform.stamp_nanos, 2 * SECOND);
    let named = found(rig.buffer, "/world", "/base", Some(2 * SECOND));
    assert_eq!(named, transform, "one leading '/' is dropped");
}

#[test]
fn a_chain_through_a_static_and_a_dynamic_transform_composes() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("tfb_chain");
    rig.send_static("base", "camera", [0.0, 0.0, 0.5]);
    rig.send_dynamic("world", "base", SECOND, [1.0, 0.0, 0.0]);
    rig.settle();
    let transform = found(rig.buffer, "world", "camera", Some(SECOND));
    assert_eq!(transform.translation, [1.0, 0.0, 0.5]);
    assert_eq!(transform.stamp_nanos, SECOND);
}

#[test]
fn a_static_transform_latched_before_the_buffer_existed_is_found_after_the_node_ran() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("tfb_late_join"));
    let tf_topic = format!("/{}", unique("tf"));
    let static_topic = format!("/{}", unique("tf_static"));
    let tf_static = tf_publisher(node, &static_topic, true, test_qos());
    // SAFETY: the publisher is live.
    unsafe { send_payload(tf_static, &tf_message("base", "lidar", 0, [0.0, 0.2, 0.0])) };

    let (status, buffer) = create_buffer(node, &tf_topic, &static_topic, &test_qos(), CACHE_NANOS);
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(
        result_of(buffer, "base", "lidar", None),
        HYPHA_TF_UNKNOWN_FRAME,
        "nothing is ingested before the node runs"
    );
    // SAFETY: `node` is live.
    let waitset = unsafe { create_waitset(node) };
    run_for(waitset, SETTLE_NANOS);
    let transform = found(buffer, "base", "lidar", None);
    assert_eq!(transform.translation, [0.0, 0.2, 0.0]);
    assert_eq!(transform.stamp_nanos, 0, "an all-static chain holds at time 0");

    // SAFETY: every handle is live and released once.
    unsafe {
        assert_eq!(hypha_transform_buffer_destroy(buffer), HyphaStatus::Ok);
        hypha_publisher_destroy(tf_static);
        hypha_node_destroy(node);
    }
}

#[test]
fn lookup_latest_takes_the_latest_time_common_to_the_chain() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("tfb_latest");
    rig.send_dynamic("world", "base", SECOND, [1.0, 0.0, 0.0]);
    rig.send_dynamic("world", "base", 4 * SECOND, [4.0, 0.0, 0.0]);
    rig.send_dynamic("base", "arm", 2 * SECOND, [0.0, 1.0, 0.0]);
    rig.send_dynamic("base", "arm", 3 * SECOND, [0.0, 1.0, 0.0]);
    rig.send_static("arm", "tool", [0.0, 0.0, 1.0]);
    rig.settle();
    let chain = found(rig.buffer, "world", "tool", None);
    assert_eq!(chain.stamp_nanos, 3 * SECOND, "the newest time both dynamic hops hold");
    assert_eq!(chain.translation, [3.0, 1.0, 1.0]);
    let all_static = found(rig.buffer, "arm", "tool", None);
    assert_eq!(all_static.stamp_nanos, 0);
    let identity = found(rig.buffer, "nowhere", "nowhere", None);
    assert_eq!(identity.translation, [0.0; 3], "equal names give the identity");
}

#[test]
fn each_refusal_has_its_own_result() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("tfb_refusals");
    rig.send_dynamic("world", "base", 5 * SECOND, [1.0, 0.0, 0.0]);
    rig.send_dynamic("world", "base", 6 * SECOND, [1.0, 0.0, 0.0]);
    rig.send_static("other", "island", [0.0, 0.0, 1.0]);
    rig.settle();
    let at = Some(5 * SECOND);
    assert_eq!(result_of(rig.buffer, "world", "nowhere", at), HYPHA_TF_UNKNOWN_FRAME);
    assert_eq!(result_of(rig.buffer, "world", "island", at), HYPHA_TF_NOT_CONNECTED);
    assert_eq!(result_of(rig.buffer, "world", "base", Some(SECOND)), HYPHA_TF_BEFORE_BUFFER);
    assert_eq!(result_of(rig.buffer, "world", "base", Some(7 * SECOND)), HYPHA_TF_AFTER_BUFFER);
    assert_eq!(result_of(rig.buffer, "", "base", at), HYPHA_TF_INVALID_FRAME);
    assert_eq!(result_of(rig.buffer, "world", "/", at), HYPHA_TF_INVALID_FRAME);

    // A refused call writes neither output.
    let mut result = 7_u32;
    let mut out = EMPTY;
    // SAFETY: the names come with their lengths; the outputs are locals.
    let latest_and_time = unsafe {
        hypha_transform_buffer_lookup(
            rig.buffer,
            "world".as_ptr().cast(),
            5,
            "base".as_ptr().cast(),
            4,
            SECOND,
            1,
            &raw mut result,
            &raw mut out,
        )
    };
    assert_eq!((latest_and_time, result), (HyphaStatus::InvalidArgument, 7));
    // SAFETY: a null buffer is refused before anything is read.
    let null_buffer = unsafe {
        hypha_transform_buffer_lookup(
            ptr::null(),
            "a".as_ptr().cast(),
            1,
            "a".as_ptr().cast(),
            1,
            0,
            1,
            &raw mut result,
            &raw mut out,
        )
    };
    assert_eq!(null_buffer, HyphaStatus::InvalidArgument);
    // SAFETY: as above, with a null output.
    let null_output = unsafe {
        hypha_transform_buffer_lookup(
            rig.buffer,
            "a".as_ptr().cast(),
            1,
            "a".as_ptr().cast(),
            1,
            0,
            1,
            ptr::null_mut(),
            &raw mut out,
        )
    };
    assert_eq!(null_output, HyphaStatus::InvalidArgument);
}

#[test]
fn a_message_that_does_not_decode_fails_no_run() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("tfb_undecodable");
    // SAFETY: the publisher is live.
    unsafe { send_payload(rig.tf, &[0xff, 0x01]) };
    assert_eq!(run_status(rig.waitset, SETTLE_NANOS), HyphaStatus::Ok as i32);
    rig.send_dynamic("world", "base", SECOND, [1.0, 0.0, 0.0]);
    rig.settle();
    assert_eq!(found(rig.buffer, "world", "base", Some(SECOND)).translation, [1.0, 0.0, 0.0]);
}

/// The handles a lookup thread and the test share, live until both threads joined.
#[derive(Clone, Copy)]
struct Shared {
    buffer: usize,
    waitset: usize,
}

static LOOKUPS_FOUND: AtomicU64 = AtomicU64::new(0);
static STOP_LOOKUPS: AtomicBool = AtomicBool::new(false);

#[test]
fn a_lookup_from_another_thread_during_the_run_succeeds_without_deadlock() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("tfb_threads");
    rig.send_dynamic("world", "base", SECOND, [0.0, 0.0, 0.0]);
    let shared = Shared { buffer: rig.buffer as usize, waitset: rig.waitset as usize };
    let runner = std::thread::spawn(move || {
        let waitset = shared.waitset as *mut HyphaWaitSet;
        let mut report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        // SAFETY: the test keeps the node and its `WaitSet` alive until this thread joined.
        let status = unsafe { hypha_waitset_run(waitset, 0, &raw mut report) };
        (status, report)
    });
    let reader = std::thread::spawn(move || {
        let buffer = shared.buffer as *const HyphaTransformBuffer;
        let mut lookups = 0_u64;
        while !STOP_LOOKUPS.load(Ordering::SeqCst) {
            let (status, result, _) = lookup(buffer, "world", "base", None);
            assert_eq!(status, HyphaStatus::Ok);
            if result == HYPHA_TF_FOUND {
                LOOKUPS_FOUND.fetch_add(1, Ordering::SeqCst);
            }
            lookups = lookups.saturating_add(1);
        }
        lookups
    });

    let started = Instant::now();
    let mut stamp = 2 * SECOND;
    while LOOKUPS_FOUND.load(Ordering::SeqCst) < 1000 {
        assert!(started.elapsed() < Duration::from_secs(10), "no lookup found the transform");
        rig.send_dynamic("world", "base", stamp, [1.0, 0.0, 0.0]);
        stamp = stamp.saturating_add(10_000_000);
        std::thread::sleep(Duration::from_millis(1));
    }
    STOP_LOOKUPS.store(true, Ordering::SeqCst);
    let lookups = reader.join().expect("the lookup thread");
    // SAFETY: the `WaitSet` is live; any thread may ask it to stop.
    assert_eq!(unsafe { hypha_waitset_request_stop(rig.waitset) }, HyphaStatus::Ok);
    let (status, report) = runner.join().expect("the run thread");
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(report.status, HyphaStatus::Ok as i32, "the run ended without a failure");
    assert_eq!(report.reason, HYPHA_RUN_REASON_STOPPED);
    assert!(lookups >= 1000);
    let latest = found(rig.buffer, "world", "base", None);
    assert!(latest.stamp_nanos > SECOND, "the run ingested while the other thread looked up");
}

static RUNNING: AtomicBool = AtomicBool::new(false);

extern "C" fn mark_running(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    RUNNING.store(true, Ordering::SeqCst);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_refused_create_writes_null_and_leaves_nothing_subscribed() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("tfb_create_refusals"));
    let tf_topic = format!("/{}", unique("tf"));
    let static_topic = format!("/{}", unique("tf_static"));
    let tf = tf_publisher(node, &tf_topic, false, test_qos());
    // `/tf_static` exists with the `default` policy, so the buffer's policy is refused.
    let tf_static = tf_publisher(node, &static_topic, true, HyphaQos::default());
    let qos = test_qos();

    let invalid = HyphaStatus::InvalidArgument;
    let refusals = [
        (create_buffer(ptr::null_mut(), &tf_topic, &static_topic, &qos, CACHE_NANOS), invalid),
        (create_buffer(node, "", &static_topic, &qos, CACHE_NANOS), invalid),
        (create_buffer(node, &tf_topic, "", &qos, CACHE_NANOS), invalid),
        (create_buffer(node, &tf_topic, &static_topic, &qos, 0), invalid),
        (create_buffer(node, &tf_topic, &tf_topic, &qos, CACHE_NANOS), invalid),
        (
            create_buffer(node, &tf_topic, &static_topic, &qos, CACHE_NANOS),
            HyphaStatus::QosMismatch,
        ),
    ];
    for ((status, buffer), expected) in refusals {
        assert_eq!(status, expected);
        assert!(buffer.is_null(), "a refused create writes null");
    }
    let mut buffer = ptr::dangling_mut::<HyphaTransformBuffer>();
    // SAFETY: the topics come with their lengths; a null `qos` is refused.
    let null_qos = unsafe {
        hypha_transform_buffer_create(
            node,
            tf_topic.as_ptr().cast(),
            tf_topic.len(),
            static_topic.as_ptr().cast(),
            static_topic.len(),
            ptr::null(),
            CACHE_NANOS,
            &raw mut buffer,
        )
    };
    assert_eq!((null_qos, buffer.is_null()), (invalid, true));
    // SAFETY: as above, with a null output.
    let null_out = unsafe {
        hypha_transform_buffer_create(
            node,
            tf_topic.as_ptr().cast(),
            tf_topic.len(),
            static_topic.as_ptr().cast(),
            static_topic.len(),
            &raw const qos,
            CACHE_NANOS,
            ptr::null_mut(),
        )
    };
    assert_eq!(null_out, invalid);
    assert_eq!(subscriber_count(tf), 0, "a refused create releases its `/tf` subscriber");
    assert_eq!(subscriber_count(tf_static), 0);

    // A running default `WaitSet` refuses the create as it refuses an attach.
    // SAFETY: `node` is live.
    let waitset = unsafe { create_waitset(node) };
    let record = crate::common::attach::timer(1_000_000, Some(mark_running), ptr::null_mut(), None);
    let mut timer = 0;
    // SAFETY: the `WaitSet` is live; `record` and `timer` are locals.
    let attached = unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut timer) };
    assert_eq!(attached, HyphaStatus::Ok);
    let other_topic = format!("/{}", unique("tf_static"));
    let handle = waitset as usize;
    let runner = std::thread::spawn(move || {
        let mut report =
            HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
        // SAFETY: the test keeps the node and its `WaitSet` alive until this thread joined.
        unsafe { hypha_waitset_run(handle as *mut HyphaWaitSet, 0, &raw mut report) }
    });
    let started = Instant::now();
    while !RUNNING.load(Ordering::SeqCst) {
        assert!(started.elapsed() < Duration::from_secs(10), "the run did not start");
        std::thread::sleep(Duration::from_millis(1));
    }
    let (running, buffer) = create_buffer(node, &tf_topic, &other_topic, &qos, CACHE_NANOS);
    assert_eq!((running, buffer.is_null()), (HyphaStatus::InvalidState, true));
    assert_eq!(subscriber_count(tf), 0, "a create refused during a run subscribes nothing");
    // SAFETY: the `WaitSet` is live; any thread may ask it to stop.
    assert_eq!(unsafe { hypha_waitset_request_stop(waitset) }, HyphaStatus::Ok);
    assert_eq!(runner.join().expect("the run thread"), HyphaStatus::Ok);

    // Nothing the refusals touched stays claimed: the same topics take a buffer now.
    let (status, buffer) = create_buffer(node, &tf_topic, &other_topic, &qos, CACHE_NANOS);
    assert_eq!(status, HyphaStatus::Ok);
    assert_eq!(subscriber_count(tf), 1);
    // SAFETY: every handle is live and released once.
    unsafe {
        assert_eq!(hypha_transform_buffer_destroy(buffer), HyphaStatus::Ok);
        hypha_publisher_destroy(tf);
        hypha_publisher_destroy(tf_static);
        hypha_node_destroy(node);
    }
}

#[test]
fn destroy_releases_both_subscribers_and_accepts_null() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: null is allowed and does nothing.
    assert_eq!(unsafe { hypha_transform_buffer_destroy(ptr::null_mut()) }, HyphaStatus::Ok);
    let rig = Rig::new("tfb_destroy");
    rig.settle();
    assert_eq!((subscriber_count(rig.tf), subscriber_count(rig.tf_static)), (1, 1));
    let (status, second) = create_buffer(
        rig.node,
        &format!("/{}", unique("tf")),
        &format!("/{}", unique("tf_static")),
        &test_qos(),
        CACHE_NANOS,
    );
    assert_eq!(status, HyphaStatus::Ok);
    // SAFETY: the second buffer is live and destroyed once.
    assert_eq!(unsafe { hypha_transform_buffer_destroy(second) }, HyphaStatus::Ok);
    rig.settle();
    assert_eq!((subscriber_count(rig.tf), subscriber_count(rig.tf_static)), (1, 1));
}

#[test]
fn destroy_then_node_destroy_and_node_destroy_then_destroy_are_both_safe() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("tfb_order"));
    let tf_topic = format!("/{}", unique("tf"));
    let static_topic = format!("/{}", unique("tf_static"));
    let tf = tf_publisher(node, &tf_topic, false, test_qos());
    let (status, buffer) = create_buffer(node, &tf_topic, &static_topic, &test_qos(), CACHE_NANOS);
    assert_eq!(status, HyphaStatus::Ok);
    // SAFETY: the buffer is live and destroyed once, before its node.
    assert_eq!(unsafe { hypha_transform_buffer_destroy(buffer) }, HyphaStatus::Ok);
    assert_eq!(subscriber_count(tf), 0, "destroy releases the `/tf` subscriber");

    let (status, outlived) =
        create_buffer(node, &tf_topic, &static_topic, &test_qos(), CACHE_NANOS);
    assert_eq!(status, HyphaStatus::Ok);
    // SAFETY: the publisher is live and released once.
    unsafe { hypha_publisher_destroy(tf) };
    // SAFETY: `node` is live and destroyed once; its `WaitSet` releases the registrations.
    unsafe { hypha_node_destroy(node) };
    // SAFETY: the buffer is live; its node is gone, which its destroy allows.
    assert_eq!(unsafe { hypha_transform_buffer_destroy(outlived) }, HyphaStatus::Ok);
}

#[test]
fn a_burst_larger_than_the_queue_is_a_counted_loss_that_fails_no_run() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = Rig::new("tfb_burst");
    // The queue holds 32 samples: the older ones of 100 are lost before the run.
    for index in 1_i64..=100 {
        rig.send_dynamic("world", "base", index.saturating_mul(SECOND / 100), [1.0, 0.0, 0.0]);
    }
    assert_eq!(run_status(rig.waitset, SETTLE_NANOS), HyphaStatus::Ok as i32);
    let newest = found(rig.buffer, "world", "base", None);
    assert_eq!(newest.stamp_nanos, SECOND, "the newest transform of the burst is ingested");
    assert_eq!(
        result_of(rig.buffer, "world", "base", Some(SECOND / 100)),
        HYPHA_TF_BEFORE_BUFFER,
        "the lost samples are not in the buffer"
    );
}

#[test]
fn under_simulated_time_every_received_sample_is_ingested() {
    // The clock is process-wide; the guard holds the serial lock.
    let _clock = crate::common::replay::SimulatedClock::start(SECOND);
    let rig = Rig::new("tfb_sim_time");
    // Stamps on both sides of simulated time: a plain buffer has no step to cut at.
    rig.send_dynamic("world", "base", SECOND / 2, [0.0, 0.0, 0.0]);
    rig.send_dynamic("world", "base", 5 * SECOND, [9.0, 0.0, 0.0]);
    rig.settle();
    assert_eq!(found(rig.buffer, "world", "base", None).stamp_nanos, 5 * SECOND);
    assert_eq!(found(rig.buffer, "world", "base", Some(SECOND / 2)).translation, [0.0; 3]);
}
