//! Transforms below the C ABI: the result codes and the `HyphaTransform` of a lookup, and
//! [`TfState`], the one ingestion and lookup code that a cycle's TF input
//! (`cycle::transforms`) and a plain node's transform buffer ([`buffer`]) both call.

pub mod buffer;

use std::fmt;
use std::time::{Duration, Instant};

use hypha_core::message::DecodeOwned as _;
use hypha_msgs::tf2_msgs::TFMessage;
use hypha_runtime::tf_buffer::{Inserted, Rigid, TfBuffer, TfError};

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
}

impl fmt::Display for TfOwner<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cycle(cycle) => write!(f, "cycle '{cycle}' TF input"),
            Self::Buffer { tf, tf_static } => {
                write!(f, "transform buffer of '{tf}' and '{tf_static}'")
            }
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

/// A transform buffer, with the count of what it refused since its last log line.
#[derive(Debug)]
pub(crate) struct TfState {
    buffer: TfBuffer,
    /// Transforms the buffer refused since the last log line.
    rejected: u64,
    /// Messages that did not decode as a `tf2_msgs/TFMessage` since the last log line.
    undecodable: u64,
    logged: Option<Instant>,
}

impl TfState {
    /// An empty buffer that keeps `cache` of dynamic history per frame.
    pub(crate) fn new(cache: Duration) -> Self {
        Self { buffer: TfBuffer::new(cache), rejected: 0, undecodable: 0, logged: None }
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
            let stamp = i64::try_from(time.as_nanos()).ok();
            let Some(stamp) = stamp.filter(|_| time.nsecs < 1_000_000_000) else {
                self.rejected = self.rejected.saturating_add(1);
                pass.refusal = Some(format!(
                    "'{child}' in '{parent}': invalid stamp {}.{} s",
                    time.secs, time.nsecs
                ));
                continue;
            };
            let t = &item.transform;
            let rigid = Rigid {
                translation: [t.translation.x, t.translation.y, t.translation.z],
                rotation_xyzw: [t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w],
            };
            match self.buffer.insert(parent, child, stamp, rigid, is_static) {
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
                    self.rejected = self.rejected.saturating_add(1);
                    pass.refusal = Some(format!("'{child}' in '{parent}': {error}"));
                }
            }
        }
        Ok(())
    }

    /// Count a message that did not decode, for an owner that goes on without it.
    pub(crate) fn count_undecodable(&mut self, pass: &mut IngestPass) {
        self.undecodable = self.undecodable.saturating_add(1);
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
                Ok(TransformLookup::Missing(code))
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
        TfState::new(Duration::from_secs(10))
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
        assert_eq!(
            tf.lookup("world", "bad", None),
            Ok(TransformLookup::Missing(HYPHA_TF_UNKNOWN_FRAME))
        );
        assert!(matches!(tf.lookup("world", "good", None), Ok(TransformLookup::Found(_))));
    }
}
