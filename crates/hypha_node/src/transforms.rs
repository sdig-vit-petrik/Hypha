//! Transforms below the C ABI: the result codes and the `HyphaTransform` of a lookup, and
//! [`TfState`], the one ingestion and lookup code that a cycle's TF input
//! (`cycle::transforms`) and a transform buffer ([`buffer`]) both call.

pub mod buffer;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hypha_core::message::DecodeOwned as _;
use hypha_msgs::tf2_msgs::TFMessage;
use hypha_runtime::tf_buffer::{Inserted, Rigid, TfBuffer, TfError};

use crate::node_health::ExecutionHealth;
use crate::status::Error;

/// The SDK default of `cache_nanos` (`hypha_cycle_bind_transforms`,
/// `hypha_transform_buffer_create`): 10 s of dynamic transforms per frame.
pub const HYPHA_DEFAULT_TF_CACHE_NANOS: u64 = 10_000_000_000;

/// The transform was found and written.
pub const HYPHA_TF_FOUND: u32 = 0;
/// A frame name is empty or only `/`.
pub const HYPHA_TF_INVALID_FRAME: u32 = 1;
/// No transform names one of the frames.
pub const HYPHA_TF_UNKNOWN_FRAME: u32 = 2;
/// Both frames are known, but no chain of transforms connects them. Never an identity.
pub const HYPHA_TF_NOT_CONNECTED: u32 = 3;
/// The time is older than a transform on the chain still holds.
pub const HYPHA_TF_BEFORE_BUFFER: u32 = 4;
/// The time is newer than the newest sample of a transform on the chain.
pub const HYPHA_TF_AFTER_BUFFER: u32 = 5;
/// The lookup failed on a frame whose newest transform the buffer refused (not finite,
/// not a unit quaternion, a stamp that is no time, a second parent): its producer is
/// broken, not late. Takes the place of `UNKNOWN_FRAME`, `NOT_CONNECTED` and
/// `AFTER_BUFFER` while that frame's newest transform is a refused one.
pub const HYPHA_TF_REFUSED: u32 = 6;

/// One looked-up transform: where `source` sits in `target`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HyphaTransform {
    /// The source origin in the target frame.
    pub translation: [f64; 3],
    /// The source orientation in the target frame, a unit quaternion `[x, y, z, w]`.
    pub rotation_xyzw: [f64; 4],
    /// The time the transform holds for, in nanoseconds. A lookup of the latest
    /// common time gets that time; 0 for an all-static chain.
    pub stamp_nanos: i64,
}

/// What a transform lookup found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransformLookup {
    /// The transform of `source` in `target`.
    Found(HyphaTransform),
    /// No transform: a `HYPHA_TF_*` value other than `HYPHA_TF_FOUND`.
    Missing(u32),
}

/// Who owns a [`TfState`], as its log lines name it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TfOwner<'a> {
    /// The TF input of the cycle of this name.
    Cycle(&'a str),
    /// A plain node's transform buffer over these `/tf` and `/tf_static` topics.
    Buffer { tf: &'a str, tf_static: &'a str },
    /// A transform buffer with no node, fed by its caller.
    Fed,
}

impl fmt::Display for TfOwner<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cycle(cycle) => write!(f, "cycle '{cycle}' TF input"),
            Self::Buffer { tf, tf_static } => {
                write!(f, "transform buffer of '{tf}' and '{tf_static}'")
            }
            Self::Fed => f.write_str("fed transform buffer"),
        }
    }
}

/// What one ingestion pass noticed, for its log lines: the frames whose history started
/// again, and the latest refusal.
#[derive(Debug, Default)]
pub(crate) struct IngestPass {
    restarted: Vec<String>,
    refusal: Option<String>,
}

/// One refusal log line per owner per second at most.
const REJECTION_LOG_INTERVAL: Duration = Duration::from_secs(1);

/// A transform buffer, with the count of what it refused since its last log line and
/// the last refusal of each frame whose newest transform was refused.
#[derive(Debug)]
pub(crate) struct TfState {
    buffer: TfBuffer,
    /// Transforms the buffer refused since the last log line.
    rejected: u64,
    /// Messages that did not decode as a `tf2_msgs/TFMessage` since the last log line.
    undecodable: u64,
    logged: Option<Instant>,
    /// The reason of the last refusal of each child frame, by its name without a leading
    /// `/`, until a transform of that frame is kept again.
    refused: HashMap<String, String>,
    /// The node's counters, where every refusal is counted for good (`tf.*` metrics).
    health: Arc<ExecutionHealth>,
}

/// The nanoseconds of a ROS stamp, or the reason it is no time.
fn stamp_nanos(secs: u32, nsecs: u32) -> Result<i64, String> {
    let time = hypha_wire::ros_time::RosTime::new(secs, nsecs);
    i64::try_from(time.as_nanos())
        .ok()
        .filter(|_| nsecs < 1_000_000_000)
        .ok_or_else(|| format!("invalid stamp {secs}.{nsecs} s"))
}

/// A transform a sender checks before it sends it: where the child sits in the parent,
/// at a ROS stamp.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HyphaSentTransform {
    /// The child origin in the parent frame.
    pub translation: [f64; 3],
    /// The child orientation in the parent frame, `[x, y, z, w]`.
    pub rotation_xyzw: [f64; 4],
    /// The header stamp, seconds part.
    pub stamp_secs: u32,
    /// The header stamp, nanoseconds part; a time needs it below 1 000 000 000.
    pub stamp_nsecs: u32,
}

/// The check every transform buffer applies to each transform it ingests, for a sender
/// to apply before it sends: `child` in `parent` (`is_static`: the stamp is not read).
/// A refusal records its reason, which names both frames, for `hypha_last_error`.
///
/// # Errors
/// `InvalidArgument` for an empty frame name, a stamp that is no time, a value that is
/// not finite or a rotation that is not a unit quaternion. A second parent or a cycle
/// needs the other transforms, so only a buffer refuses those.
pub fn transform_check(
    parent: &str,
    child: &str,
    transform: &HyphaSentTransform,
    is_static: bool,
) -> Result<(), Error> {
    let rigid =
        Rigid { translation: transform.translation, rotation_xyzw: transform.rotation_xyzw };
    let stamp = (transform.stamp_secs, transform.stamp_nsecs);
    refusal_of(parent, child, stamp, rigid, is_static).map_err(|reason| {
        crate::last_error::record(&reason);
        Error::InvalidArgument
    })
}

/// The reason [`transform_check`] refuses a transform, naming the child and the parent.
fn refusal_of(
    parent: &str,
    child: &str,
    stamp: (u32, u32),
    rigid: Rigid,
    is_static: bool,
) -> Result<(), String> {
    let named = |reason: &dyn fmt::Display| format!("'{child}' in '{parent}': {reason}");
    let nanos = if is_static {
        0
    } else {
        stamp_nanos(stamp.0, stamp.1).map_err(|reason| named(&reason))?
    };
    hypha_runtime::tf_buffer::check(parent, child, nanos, rigid, is_static)
        .map_err(|error| named(&error))
}

/// `name` without its one leading `/`.
fn frame_key(name: &str) -> &str {
    name.strip_prefix('/').unwrap_or(name)
}

impl TfState {
    /// An empty buffer that keeps `cache` of dynamic history per frame, counting its
    /// refusals in `health`.
    pub(crate) fn new(cache: Duration, health: Arc<ExecutionHealth>) -> Self {
        Self {
            buffer: TfBuffer::new(cache),
            rejected: 0,
            undecodable: 0,
            logged: None,
            refused: HashMap::new(),
            health,
        }
    }

    /// Count and keep one refusal of `child`.
    fn refuse(&mut self, child: &str, reason: String, pass: &mut IngestPass) {
        self.rejected = self.rejected.saturating_add(1);
        self.health.count_tf_refused();
        self.refused.insert(frame_key(child).to_owned(), reason.clone());
        pass.refusal = Some(reason);
    }

    /// Insert every transform of the `tf2_msgs/TFMessage` in `payload`, as static ones
    /// when `is_static`. A transform the buffer refuses, or whose header stamp is not a
    /// time, is counted and left out; the others are kept.
    ///
    /// # Errors
    /// `InvalidFrame` when `payload` does not decode; nothing is inserted then.
    pub(crate) fn ingest(
        &mut self,
        owner: TfOwner<'_>,
        payload: &[u8],
        is_static: bool,
        pass: &mut IngestPass,
    ) -> Result<(), Error> {
        let message = TFMessage::decode_from(payload).map_err(|_| Error::InvalidFrame)?;
        for item in &message.transforms {
            let parent = &item.header.frame_id;
            let child = &item.child_frame_id;
            let time = item.header.stamp;
            let stamp = match stamp_nanos(time.secs, time.nsecs) {
                Ok(stamp) => stamp,
                Err(reason) => {
                    self.refuse(child, format!("'{child}' in '{parent}': {reason}"), pass);
                    continue;
                }
            };
            let t = &item.transform;
            let rigid = Rigid {
                translation: [t.translation.x, t.translation.y, t.translation.z],
                rotation_xyzw: [t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w],
            };
            let inserted = self.buffer.insert(parent, child, stamp, rigid, is_static);
            if inserted.is_ok() {
                self.refused.remove(frame_key(child));
            }
            match inserted {
                Ok(Inserted::Kept) => {}
                Ok(Inserted::DroppedHistory { newest_nanos }) => {
                    let first_time_this_pass = !pass.restarted.contains(child);
                    if first_time_this_pass {
                        log_dropped_history(
                            owner,
                            parent,
                            child,
                            stamp,
                            newest_nanos,
                            self.buffer.cache_nanos(),
                        );
                        pass.restarted.push(child.clone());
                    }
                }
                Err(error) => {
                    self.refuse(child, format!("'{child}' in '{parent}': {error}"), pass);
                }
            }
        }
        Ok(())
    }

    /// Forget every dynamic transform and return how many frames held one. Static
    /// transforms, and the refusal kept for a frame, stay.
    pub(crate) fn clear_dynamic(&mut self) -> usize {
        self.buffer.clear_dynamic()
    }

    /// Count a message that did not decode, for an owner that goes on without it.
    pub(crate) fn count_undecodable(&mut self, pass: &mut IngestPass) {
        self.undecodable = self.undecodable.saturating_add(1);
        self.health.count_tf_undecodable();
        pass.refusal = Some("a message that does not decode as tf2_msgs/TFMessage".to_owned());
    }

    /// End a pass: log what it refused, at most once per [`REJECTION_LOG_INTERVAL`].
    pub(crate) fn finish(&mut self, owner: TfOwner<'_>, pass: IngestPass) {
        let Some(latest) = pass.refusal else { return };
        let now = Instant::now();
        if self.logged.is_some_and(|at| now.duration_since(at) < REJECTION_LOG_INTERVAL) {
            return;
        }
        if self.undecodable == 0 {
            log::error!(target: "hypha", "hypha: {owner} refused {} transform(s), latest {latest}", self.rejected);
        } else {
            log::error!(target: "hypha", "hypha: {owner} refused {} transform(s) and {} undecodable message(s), latest {latest}", self.rejected, self.undecodable);
        }
        self.rejected = 0;
        self.undecodable = 0;
        self.logged = Some(now);
    }

    /// Look up `source` in `target` at `at` nanoseconds, or at the latest time common to
    /// the chain for `None`.
    ///
    /// # Errors
    /// `InvalidState` for an overflow composing the chain.
    pub(crate) fn lookup(
        &self,
        target: &str,
        source: &str,
        at: Option<i64>,
    ) -> Result<TransformLookup, Error> {
        match self.buffer.lookup(target, source, at) {
            Ok((rigid, stamp_nanos)) => Ok(TransformLookup::Found(HyphaTransform {
                translation: rigid.translation,
                rotation_xyzw: rigid.rotation_xyzw,
                stamp_nanos,
            })),
            Err(error) => {
                let code = result_code(&error).ok_or(Error::InvalidState)?;
                let refusal_can_explain = matches!(
                    error,
                    TfError::UnknownFrame(_) | TfError::NotConnected | TfError::AfterBuffer
                );
                let refused = refusal_can_explain
                    && self
                        .buffer
                        .frames_above(target, source)
                        .iter()
                        .any(|name| self.refused.contains_key(name));
                Ok(TransformLookup::Missing(if refused { HYPHA_TF_REFUSED } else { code }))
            }
        }
    }
}

/// Logs that `child` in `parent` came more than the buffer length before its newest
/// transform, and that its older transforms were dropped.
fn log_dropped_history(
    owner: TfOwner<'_>,
    parent: &str,
    child: &str,
    stamp: i64,
    newest_nanos: i64,
    cache_nanos: i64,
) {
    let behind = seconds_text(newest_nanos.saturating_sub(stamp));
    let cache = seconds_text(cache_nanos);
    log::error!(target: "hypha", "hypha: {owner}: '{child}' in '{parent}' is stamped {behind} s before its newest transform, more than the {cache} s buffer: its older transforms are dropped");
}

/// `nanos` as seconds with three decimals, in integer arithmetic.
fn seconds_text(nanos: i64) -> String {
    let millis = nanos / 1_000_000;
    format!("{}.{:03}", millis / 1000, (millis % 1000).abs())
}

fn result_code(error: &TfError) -> Option<u32> {
    match error {
        TfError::InvalidFrame(_) => Some(HYPHA_TF_INVALID_FRAME),
        TfError::UnknownFrame(_) => Some(HYPHA_TF_UNKNOWN_FRAME),
        TfError::NotConnected => Some(HYPHA_TF_NOT_CONNECTED),
        TfError::BeforeBuffer => Some(HYPHA_TF_BEFORE_BUFFER),
        TfError::AfterBuffer => Some(HYPHA_TF_AFTER_BUFFER),
        TfError::Rejected(_) => None,
    }
}

#[cfg(test)]
mod tests {
    //! The shared ingestion.
    #![expect(clippy::expect_used, reason = "a failed test setup stops the test")]

    use super::*;
    use hypha_core::message::EncodeInto as _;
    use hypha_msgs::geometry_msgs::{Quaternion, Transform, TransformStamped, Vector3};
    use hypha_msgs::std_msgs::Header;
    use hypha_wire::ros_time::RosTime;

    const OWNER: TfOwner<'static> = TfOwner::Buffer { tf: "/tf", tf_static: "/tf_static" };

    fn transform(parent: &str, child: &str, stamp: RosTime, x: f64) -> TransformStamped {
        TransformStamped {
            header: Header { seq: 0, stamp, frame_id: parent.into() },
            child_frame_id: child.into(),
            transform: Transform {
                translation: Vector3 { x, y: 0.0, z: 0.0 },
                rotation: Quaternion { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
            },
        }
    }

    fn encoded(transforms: Vec<TransformStamped>) -> Vec<u8> {
        let message = TFMessage { transforms };
        let mut payload = vec![0_u8; message.encoded_len()];
        message.encode_into(&mut payload).expect("a TFMessage encodes");
        payload
    }

    fn state() -> TfState {
        TfState::new(Duration::from_secs(10), Arc::default())
    }

    #[test]
    fn an_ingested_message_is_found_by_a_lookup() {
        let mut tf = state();
        let mut pass = IngestPass::default();
        let payload = encoded(vec![
            transform("world", "base", RosTime::new(1, 0), 1.0),
            transform("base", "arm", RosTime::new(1, 0), 2.0),
        ]);
        tf.ingest(OWNER, &payload, false, &mut pass).expect("the message decodes");
        assert!(pass.refusal.is_none());
        let expected = HyphaTransform {
            translation: [3.0, 0.0, 0.0],
            rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
            stamp_nanos: 1_000_000_000,
        };
        assert_eq!(
            tf.lookup("/world", "arm", Some(1_000_000_000)),
            Ok(TransformLookup::Found(expected))
        );
    }

    #[test]
    fn a_payload_that_does_not_decode_inserts_nothing() {
        let mut tf = state();
        let mut pass = IngestPass::default();
        assert_eq!(tf.ingest(OWNER, &[0xff, 0x01], false, &mut pass), Err(Error::InvalidFrame));
        assert_eq!(
            tf.lookup("world", "base", None),
            Ok(TransformLookup::Missing(HYPHA_TF_UNKNOWN_FRAME))
        );
        tf.count_undecodable(&mut pass);
        assert_eq!(tf.undecodable, 1);
        tf.finish(OWNER, pass);
        assert_eq!(tf.undecodable, 0, "the log line reports and clears the count");
    }

    const REFUSED: u32 = HYPHA_TF_REFUSED;

    fn nan_transform(parent: &str, child: &str, stamp: RosTime) -> TransformStamped {
        transform(parent, child, stamp, f64::NAN)
    }

    #[test]
    fn a_lookup_that_fails_on_a_refused_frame_answers_refused() {
        let mut tf = state();
        let mut pass = IngestPass::default();
        let good = encoded(vec![transform("map", "base", RosTime::new(1, 0), 1.0)]);
        tf.ingest(OWNER, &good, false, &mut pass).expect("the message decodes");
        let broken = encoded(vec![nan_transform("map", "base", RosTime::new(2, 0))]);
        tf.ingest(OWNER, &broken, false, &mut pass).expect("the message decodes");
        let never = encoded(vec![nan_transform("map", "gnss", RosTime::new(2, 0))]);
        tf.ingest(OWNER, &never, false, &mut pass).expect("the message decodes");
        // The good sample still answers at its own time.
        assert!(matches!(
            tf.lookup("map", "base", Some(1_000_000_000)),
            Ok(TransformLookup::Found(_))
        ));
        // After it, and for a frame never accepted, the answer is the refusal, not "not yet".
        assert_eq!(
            tf.lookup("map", "base", Some(2_000_000_000)),
            Ok(TransformLookup::Missing(REFUSED))
        );
        assert_eq!(tf.lookup("map", "gnss", None), Ok(TransformLookup::Missing(REFUSED)));
        // The log line resets its own count, never the node's metric.
        tf.finish(OWNER, pass);
        tf.count_undecodable(&mut IngestPass::default());
        let mut metrics = Vec::new();
        tf.health.emit_tf_metrics(&mut |name, value| metrics.push((name.to_owned(), value)));
        assert_eq!(metrics, vec![("tf.refused".to_owned(), 2), ("tf.undecodable".to_owned(), 1)]);
    }

    #[test]
    fn the_sender_check_names_the_frames_and_the_reason() {
        let finite = Rigid { translation: [0.0; 3], rotation_xyzw: [0.0, 0.0, 0.0, 1.0] };
        let broken = Rigid { translation: [f64::NAN, 0.0, 0.0], ..finite };
        assert_eq!(refusal_of("map", "base", (1, 0), finite, false), Ok(()));
        let reason = refusal_of("map", "base", (1, 0), broken, false).expect_err("NaN");
        assert!(reason.starts_with("'base' in 'map': transform rejected:"), "{reason}");
        let stamp = refusal_of("map", "base", (1, 1_000_000_000), finite, false);
        assert_eq!(stamp, Err("'base' in 'map': invalid stamp 1.1000000000 s".to_owned()));
        assert_eq!(refusal_of("map", "base", (1, 1_000_000_000), finite, true), Ok(()));
        assert!(refusal_of("", "base", (1, 0), finite, false).is_err());
        let sent = HyphaSentTransform {
            translation: [f64::NAN, 0.0, 0.0],
            rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
            stamp_secs: 1,
            stamp_nsecs: 0,
        };
        assert_eq!(transform_check("map", "base", &sent, false), Err(Error::InvalidArgument));
        assert!(crate::last_error::last_error().starts_with("'base' in 'map': "));
    }

    #[test]
    fn a_refused_frame_on_the_chain_answers_refused_and_an_accepted_one_clears_it() {
        let mut tf = state();
        let mut pass = IngestPass::default();
        let setup = encoded(vec![
            transform("map", "base", RosTime::new(1, 0), 1.0),
            transform("base", "lidar", RosTime::new(1, 0), 0.5),
            transform("base", "lidar", RosTime::new(3, 0), 0.5),
        ]);
        tf.ingest(OWNER, &setup, false, &mut pass).expect("the message decodes");
        let broken = encoded(vec![nan_transform("map", "base", RosTime::new(2, 0))]);
        tf.ingest(OWNER, &broken, false, &mut pass).expect("the message decodes");
        assert_eq!(
            tf.lookup("map", "lidar", Some(2_000_000_000)),
            Ok(TransformLookup::Missing(REFUSED))
        );
        let fixed = encoded(vec![transform("map", "base", RosTime::new(3, 0), 1.0)]);
        tf.ingest(OWNER, &fixed, false, &mut pass).expect("the message decodes");
        assert!(matches!(
            tf.lookup("map", "lidar", Some(2_000_000_000)),
            Ok(TransformLookup::Found(_))
        ));
        // A lookup after the buffer of a frame that was never refused stays AfterBuffer.
        assert_eq!(
            tf.lookup("map", "lidar", Some(9_000_000_000)),
            Ok(TransformLookup::Missing(HYPHA_TF_AFTER_BUFFER))
        );
    }

    #[test]
    fn a_stamp_that_is_no_time_is_counted_and_left_out() {
        let mut tf = state();
        let mut pass = IngestPass::default();
        let payload = encoded(vec![
            transform("world", "bad", RosTime::new(1, 1_000_000_000), 1.0),
            transform("world", "good", RosTime::new(1, 0), 1.0),
        ]);
        tf.ingest(OWNER, &payload, false, &mut pass).expect("the message decodes");
        assert_eq!(tf.rejected, 1);
        assert!(pass.refusal.is_some_and(|text| text.contains("invalid stamp")));
        // Left out, and the lookup says why: the frame's transform was refused (#123).
        assert_eq!(tf.lookup("world", "bad", None), Ok(TransformLookup::Missing(HYPHA_TF_REFUSED)));
        assert_eq!(
            tf.lookup("world", "nowhere", None),
            Ok(TransformLookup::Missing(HYPHA_TF_UNKNOWN_FRAME))
        );
        assert!(matches!(tf.lookup("world", "good", None), Ok(TransformLookup::Found(_))));
    }
}
