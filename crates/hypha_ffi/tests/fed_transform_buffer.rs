//! The transform buffer with no node through the C ABI: fed by its caller, it answers
//! as a cycle's TF input does.
#![expect(clippy::expect_used, reason = "a failed test setup stops the test with its message")]

use std::ptr;

use hypha_core::message::EncodeInto as _;
use hypha_ffi::*;
use hypha_msgs::geometry_msgs::{Quaternion, Transform, TransformStamped, Vector3};
use hypha_msgs::std_msgs::Header;
use hypha_msgs::tf2_msgs::TFMessage;
use hypha_wire::ros_time::RosTime;

const SECOND: i64 = 1_000_000_000;
const CACHE_NANOS: u64 = 10_000_000_000;
const EMPTY: HyphaTransform =
    HyphaTransform { translation: [0.0; 3], rotation_xyzw: [0.0; 4], stamp_nanos: 0 };

/// `child` in `parent` at `seconds`, translated by `x`, with the rotation's `w`.
fn stamped(parent: &str, child: &str, seconds: u32, x: f64, w: f64) -> TransformStamped {
    TransformStamped {
        header: Header { seq: 0, stamp: RosTime::new(seconds, 0), frame_id: parent.into() },
        child_frame_id: child.into(),
        transform: Transform {
            translation: Vector3 { x, y: 0.0, z: 0.0 },
            rotation: Quaternion { x: 0.0, y: 0.0, z: 0.0, w },
        },
    }
}

fn payload(transforms: Vec<TransformStamped>) -> Vec<u8> {
    let message = TFMessage { transforms };
    let mut bytes = vec![0_u8; message.encoded_len()];
    message.encode_into(&mut bytes).expect("a TFMessage encodes");
    bytes
}

/// A fed buffer, destroyed on drop.
struct Fed(*mut HyphaTransformBuffer);

impl Fed {
    fn new() -> Self {
        let mut buffer = ptr::null_mut();
        // SAFETY: `buffer` is a local.
        let status = unsafe { hypha_transform_buffer_create_fed(CACHE_NANOS, &raw mut buffer) };
        assert_eq!(status, HyphaStatus::Ok);
        Self(buffer)
    }

    fn feed(&self, bytes: &[u8], is_static: bool) -> HyphaStatus {
        // SAFETY: the buffer is live; the payload comes with its length.
        unsafe {
            hypha_transform_buffer_feed(self.0, bytes.as_ptr(), bytes.len(), u32::from(is_static))
        }
    }

    fn lookup(&self, target: &str, source: &str, at: Option<i64>) -> (u32, HyphaTransform) {
        let mut result = u32::MAX;
        let mut out = EMPTY;
        // SAFETY: the buffer is live; the names come with their lengths; the outputs are locals.
        let status = unsafe {
            hypha_transform_buffer_lookup(
                self.0,
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
        assert_eq!(status, HyphaStatus::Ok);
        (result, out)
    }

    fn result(&self, target: &str, source: &str, at: Option<i64>) -> u32 {
        self.lookup(target, source, at).0
    }
}

impl Drop for Fed {
    fn drop(&mut self) {
        // SAFETY: the buffer is live and used no more.
        assert_eq!(unsafe { hypha_transform_buffer_destroy(self.0) }, HyphaStatus::Ok);
    }
}

#[test]
fn a_dynamic_and_a_static_transform_compose_and_a_stamp_between_two_interpolates() {
    let fed = Fed::new();
    let dynamic =
        payload(vec![stamped("map", "base", 1, 1.0, 1.0), stamped("map", "base", 3, 3.0, 1.0)]);
    assert_eq!(fed.feed(&dynamic, false), HyphaStatus::Ok);
    assert_eq!(
        fed.feed(&payload(vec![stamped("base", "lidar", 0, 0.5, 1.0)]), true),
        HyphaStatus::Ok
    );
    let (result, found) = fed.lookup("map", "lidar", Some(2 * SECOND));
    assert_eq!(result, HYPHA_TF_FOUND);
    assert_eq!(found.translation[0].to_bits(), 2.5_f64.to_bits(), "halfway, then the static");
    assert_eq!(found.stamp_nanos, 2 * SECOND);
    let (latest, newest) = fed.lookup("map", "lidar", None);
    assert_eq!((latest, newest.stamp_nanos), (HYPHA_TF_FOUND, 3 * SECOND));
}

#[test]
fn a_lookup_misses_as_a_cycle_input_does() {
    let fed = Fed::new();
    assert_eq!(fed.result("map", "base", None), HYPHA_TF_UNKNOWN_FRAME);
    fed.feed(&payload(vec![stamped("map", "base", 5, 1.0, 1.0)]), false);
    fed.feed(&payload(vec![stamped("map", "base", 6, 2.0, 1.0)]), false);
    assert_eq!(fed.result("map", "base", Some(4 * SECOND)), HYPHA_TF_BEFORE_BUFFER);
    assert_eq!(fed.result("map", "base", Some(7 * SECOND)), HYPHA_TF_AFTER_BUFFER);
    fed.feed(&payload(vec![stamped("odom", "wheel", 5, 1.0, 1.0)]), false);
    assert_eq!(fed.result("map", "wheel", Some(5 * SECOND)), HYPHA_TF_NOT_CONNECTED);
    assert_eq!(fed.result("", "base", None), HYPHA_TF_INVALID_FRAME);
}

#[test]
fn a_refused_transform_is_left_out_and_its_frame_answers_refused() {
    let fed = Fed::new();
    // A rotation that is no unit quaternion.
    let fed_status = fed.feed(&payload(vec![stamped("map", "base", 1, 1.0, 0.0)]), false);
    assert_eq!(fed_status, HyphaStatus::Ok, "a refused transform fails no feed");
    assert_eq!(fed.result("map", "base", Some(SECOND)), HYPHA_TF_REFUSED);
}

#[test]
fn a_payload_that_does_not_decode_is_refused_and_inserts_nothing() {
    let fed = Fed::new();
    assert_eq!(fed.feed(&[0xff, 0x01], false), HyphaStatus::InvalidFrame);
    assert_eq!(fed.result("map", "base", None), HYPHA_TF_UNKNOWN_FRAME);
}

#[test]
fn the_arguments_are_checked_and_a_refused_call_writes_nothing() {
    let mut buffer = ptr::dangling_mut::<HyphaTransformBuffer>();
    // SAFETY: `buffer` is a local.
    let zero_cache = unsafe { hypha_transform_buffer_create_fed(0, &raw mut buffer) };
    assert_eq!(zero_cache, HyphaStatus::InvalidArgument);
    assert!(buffer.is_null(), "a refused create writes null");
    // SAFETY: a null out is refused.
    let no_out = unsafe { hypha_transform_buffer_create_fed(1, ptr::null_mut()) };
    assert_eq!(no_out, HyphaStatus::InvalidArgument);

    let fed = Fed::new();
    // SAFETY: a null payload with a length is refused before anything is read.
    let no_payload = unsafe { hypha_transform_buffer_feed(fed.0, ptr::null(), 1, 0) };
    assert_eq!(no_payload, HyphaStatus::InvalidArgument);
    // SAFETY: a null buffer is refused before the payload is read.
    let no_buffer = unsafe { hypha_transform_buffer_feed(ptr::null(), [0_u8].as_ptr(), 1, 0) };
    assert_eq!(no_buffer, HyphaStatus::InvalidArgument);
    assert_eq!(fed.feed(&[], false), HyphaStatus::InvalidFrame, "no bytes are no TFMessage");
}
