//! The coordinate-frame buffer behind a cycle's TF input, over the `transforms` crate.
//!
//! Frame names lose one leading `/` and must not be empty, time 0 asks for the latest
//! time common to the chain, and no lookup waits.

use std::time::Duration;

use transforms::Registry;
use transforms::errors::RegistryError;
use transforms::geometry::{Quaternion, Transform, Vector3};
use transforms::time::{Stamp, Timestamp};

/// A rigid transform: where a child frame sits in its parent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rigid {
    /// The child origin in the parent frame.
    pub translation: [f64; 3],
    /// The child orientation in the parent frame, a unit quaternion `[x, y, z, w]`.
    pub rotation_xyzw: [f64; 4],
}

/// Why an insert or a lookup failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TfError {
    /// A frame name is empty, or only `/`. Holds the name as given.
    InvalidFrame(String),
    /// No transform names this frame. Holds the name without its leading `/`.
    UnknownFrame(String),
    /// Both frames are known, but no chain of transforms connects them.
    NotConnected,
    /// The time is older than what a transform on the chain still holds.
    BeforeBuffer,
    /// The time is newer than the newest sample of a transform on the chain.
    AfterBuffer,
    /// The buffer refused the transform (not a unit quaternion, non-finite, negative
    /// stamp, second parent, cycle, static and dynamic). Holds the reason.
    Rejected(String),
}

impl std::fmt::Display for TfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFrame(name) => write!(f, "invalid frame name '{name}'"),
            Self::UnknownFrame(name) => write!(f, "unknown frame '{name}'"),
            Self::NotConnected => f.write_str("the frames are not connected"),
            Self::BeforeBuffer => f.write_str("the time is before the buffer"),
            Self::AfterBuffer => f.write_str("the time is after the buffer"),
            Self::Rejected(reason) => write!(f, "transform rejected: {reason}"),
        }
    }
}

impl std::error::Error for TfError {}

/// The transforms one TF input holds. Dynamic samples older than the cache length
/// before a frame's newest sample are evicted on insert; static transforms stay.
#[derive(Debug)]
pub struct TfBuffer {
    registry: Registry,
    /// The history length, in nanoseconds.
    cache_nanos: i64,
    /// The newest dynamic stamp of each child frame, by its name without a leading `/`.
    newest: std::collections::HashMap<String, i64>,
    /// The parent of each child frame the buffer holds, by names without a leading `/`.
    parents: std::collections::HashMap<String, String>,
}

/// What an insert did with a frame's history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inserted {
    /// The transform joined the history.
    Kept,
    /// The transform was more than the cache length older than the frame's newest stamp:
    /// the frame's older transforms were dropped first.
    DroppedHistory {
        /// The newest stamp of the frame before the insert.
        newest_nanos: i64,
    },
}

/// A frame name without its one leading `/`, or `InvalidFrame` for an empty name.
fn frame(name: &str) -> Result<&str, TfError> {
    let stripped = name.strip_prefix('/').unwrap_or(name);
    if stripped.is_empty() {
        return Err(TfError::InvalidFrame(name.to_owned()));
    }
    Ok(stripped)
}

fn timestamp(nanos: i64) -> Option<Timestamp> {
    u64::try_from(nanos).ok().map(Timestamp::from_nanos)
}

fn outside(requested: Option<Timestamp>, covered: Option<(Timestamp, Timestamp)>) -> TfError {
    match (requested, covered) {
        (Some(time), Some((start, _))) if time < start => TfError::BeforeBuffer,
        _ => TfError::AfterBuffer,
    }
}

fn lookup_error(error: RegistryError) -> TfError {
    match error {
        RegistryError::UnknownFrame(name) => TfError::UnknownFrame(name),
        RegistryError::Disconnected { .. } => TfError::NotConnected,
        RegistryError::NotFoundAt { requested, covered, .. } => outside(Some(requested), covered),
        // No instant serves every hop.
        RegistryError::NoCommonTime { covered: Some(_), .. } => TfError::BeforeBuffer,
        RegistryError::NoCommonTime { covered: None, .. } => TfError::AfterBuffer,
        other => TfError::Rejected(other.to_string()),
    }
}

/// A transform made from its parts after the check every transform passes: both frame
/// names valid, a stamp that is a time unless `is_static`, finite values and a unit
/// quaternion. Returns the frame names without a leading `/`.
fn checked<'a>(
    parent: &'a str,
    child: &'a str,
    stamp_nanos: i64,
    rigid: Rigid,
    is_static: bool,
) -> Result<(&'a str, &'a str, Transform), TfError> {
    let parent = frame(parent)?;
    let child = frame(child)?;
    let stamp = if is_static {
        Stamp::Static
    } else {
        let time = timestamp(stamp_nanos)
            .ok_or_else(|| TfError::Rejected(format!("negative stamp {stamp_nanos}")))?;
        Stamp::At(time)
    };
    let [x, y, z] = rigid.translation;
    let [qx, qy, qz, qw] = rigid.rotation_xyzw;
    let transform = Transform::new(
        parent,
        child,
        Vector3::new(x, y, z),
        Quaternion::from_wxyz(qw, qx, qy, qz),
        stamp,
    )
    .map_err(|error| TfError::Rejected(error.to_string()))?;
    Ok((parent, child, transform))
}

/// The check a buffer applies to every transform it is given, for a sender to apply
/// before it sends: `child` in `parent` at `stamp_nanos` (not read when `is_static`).
/// It needs no buffer, so it cannot see a second parent or a cycle, which only a
/// buffer that holds the other transforms refuses.
///
/// # Errors
/// `InvalidFrame` for an empty name, `Rejected` with the reason for a negative stamp, a
/// non-finite value or a rotation that is not a unit quaternion.
pub fn check(
    parent: &str,
    child: &str,
    stamp_nanos: i64,
    rigid: Rigid,
    is_static: bool,
) -> Result<(), TfError> {
    checked(parent, child, stamp_nanos, rigid, is_static).map(|_| ())
}

impl TfBuffer {
    /// An empty buffer that keeps `cache` of dynamic history per frame.
    #[must_use]
    pub fn new(cache: Duration) -> Self {
        let cache_nanos = i64::try_from(cache.as_nanos()).unwrap_or(i64::MAX);
        Self {
            registry: Registry::with_max_age(cache),
            cache_nanos,
            newest: std::collections::HashMap::new(),
            parents: std::collections::HashMap::new(),
        }
    }

    /// `target`, `source` and every frame above either, by the parents the buffer holds,
    /// without a leading `/`: the frames a chain between the two can pass. An empty name
    /// is left out.
    #[must_use]
    pub fn frames_above(&self, target: &str, source: &str) -> Vec<String> {
        let mut frames: Vec<String> = Vec::new();
        for start in [target, source] {
            let Ok(mut name) = frame(start) else { continue };
            // A frame keeps one parent and the buffer refuses a cycle; the bound guards anyway.
            for _ in 0..=self.parents.len() {
                if !frames.iter().any(|known| known == name) {
                    frames.push(name.to_owned());
                }
                let Some(parent) = self.parents.get(name) else { break };
                name = parent;
            }
        }
        frames
    }

    /// The history length of each frame, in nanoseconds.
    #[must_use]
    pub fn cache_nanos(&self) -> i64 {
        self.cache_nanos
    }

    /// Insert one transform of a `tf2_msgs/TFMessage`: `child` in `parent` at the
    /// header stamp `stamp_nanos`. A static transform holds for all time.
    ///
    ///
    /// A dynamic transform more than the cache length older than the frame's newest one
    /// drops the frame's older transforms first ([`Inserted::DroppedHistory`]).
    ///
    /// # Errors
    /// `InvalidFrame` for an empty name, `Rejected` for a transform the buffer refuses:
    /// one [`check`] refuses, a second parent, a cycle.
    pub fn insert(
        &mut self,
        parent: &str,
        child: &str,
        stamp_nanos: i64,
        rigid: Rigid,
        is_static: bool,
    ) -> Result<Inserted, TfError> {
        let (parent, child, transform) = checked(parent, child, stamp_nanos, rigid, is_static)?;
        let mut inserted = Inserted::Kept;
        if !is_static && let Some(newest_nanos) = self.newest.get(child).copied() {
            let behind = newest_nanos.saturating_sub(stamp_nanos);
            if behind > self.cache_nanos {
                self.registry.remove_frame(child);
                self.newest.remove(child);
                inserted = Inserted::DroppedHistory { newest_nanos };
            }
        }
        self.registry
            .add_transform(transform)
            .map_err(|error| TfError::Rejected(error.to_string()))?;
        self.parents.insert(child.to_owned(), parent.to_owned());
        if !is_static {
            let newest = self.newest.entry(child.to_owned()).or_insert(stamp_nanos);
            *newest = (*newest).max(stamp_nanos);
        }
        Ok(inserted)
    }

    /// The transform that maps `source` coordinates into `target` at `at_nanos`, and
    /// the time it holds for. `None` asks for the latest time common to every transform
    /// on the chain; an all-static chain then holds at time 0.
    ///
    /// # Errors
    /// Every `TfError` but `Rejected`, which only an arithmetic overflow returns here.
    pub fn lookup(
        &self,
        target: &str,
        source: &str,
        at_nanos: Option<i64>,
    ) -> Result<(Rigid, i64), TfError> {
        let target = frame(target)?;
        let source = frame(source)?;
        let time = match at_nanos {
            None => match self.registry.latest_common_time(target, source).map_err(lookup_error)? {
                Stamp::At(time) => time,
                Stamp::Static => Timestamp::zero(),
            },
            Some(at) => timestamp(at).ok_or(TfError::BeforeBuffer)?,
        };
        let transform = self.registry.get_transform(target, source, time).map_err(|error| {
            // The crate reports a sampling failure before a disconnection or an unknown frame.
            match (lookup_error(error), self.registry.latest_common_time(target, source)) {
                (
                    TfError::BeforeBuffer | TfError::AfterBuffer,
                    Err(
                        topology @ (RegistryError::UnknownFrame(_)
                        | RegistryError::Disconnected { .. }),
                    ),
                ) => lookup_error(topology),
                (sampling, _) => sampling,
            }
        })?;
        let translation = transform.translation();
        let rotation = transform.rotation();
        let rigid = Rigid {
            translation: [translation.x, translation.y, translation.z],
            rotation_xyzw: [rotation.x, rotation.y, rotation.z, rotation.w],
        };
        let nanos = i64::try_from(time.as_nanos()).map_err(|_| TfError::AfterBuffer)?;
        Ok((rigid, nanos))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    const SECOND: i64 = 1_000_000_000;
    const IDENTITY: [f64; 4] = [0.0, 0.0, 0.0, 1.0];

    fn at(x: f64, y: f64, z: f64) -> Rigid {
        Rigid { translation: [x, y, z], rotation_xyzw: IDENTITY }
    }

    fn buffer() -> TfBuffer {
        TfBuffer::new(std::time::Duration::from_secs(10))
    }

    #[test]
    fn a_chain_composes() {
        let mut tf = buffer();
        tf.insert("world", "base", SECOND, at(1.0, 0.0, 0.0), false).unwrap();
        tf.insert("base", "sensor", 0, at(0.0, 1.0, 0.0), true).unwrap();
        let (rigid, _) = tf.lookup("world", "sensor", Some(SECOND)).unwrap();
        assert_eq!(rigid.translation, [1.0, 1.0, 0.0]);
    }

    #[test]
    fn dynamic_samples_interpolate() {
        let mut tf = buffer();
        tf.insert("world", "base", 0, at(0.0, 0.0, 0.0), false).unwrap();
        tf.insert("world", "base", 10 * SECOND, at(10.0, 0.0, 0.0), false).unwrap();
        let (rigid, time) = tf.lookup("world", "base", Some(5 * SECOND)).unwrap();
        assert!((rigid.translation[0] - 5.0).abs() < 1e-9, "{rigid:?}");
        assert_eq!(time, 5 * SECOND);
    }

    #[test]
    fn time_zero_is_a_time_like_any_other() {
        let mut tf = buffer();
        tf.insert("world", "base", 0, at(0.0, 0.0, 0.0), false).unwrap();
        tf.insert("world", "base", SECOND, at(1.0, 0.0, 0.0), false).unwrap();
        let (rigid, time) = tf.lookup("world", "base", Some(0)).unwrap();
        assert_eq!((rigid.translation, time), ([0.0; 3], 0), "simulated time can start at 0");
    }

    #[test]
    fn no_time_is_the_latest_common_time() {
        let mut tf = buffer();
        tf.insert("world", "base", 5 * SECOND, at(5.0, 0.0, 0.0), false).unwrap();
        tf.insert("world", "base", 10 * SECOND, at(10.0, 0.0, 0.0), false).unwrap();
        tf.insert("base", "sensor", 0, at(0.0, 1.0, 0.0), true).unwrap();
        let (rigid, time) = tf.lookup("world", "sensor", None).unwrap();
        assert_eq!(time, 10 * SECOND);
        assert_eq!(rigid.translation, [10.0, 1.0, 0.0]);
    }

    #[test]
    fn an_all_static_chain_holds_at_time_zero_when_latest() {
        let mut tf = buffer();
        tf.insert("base", "sensor", 0, at(0.0, 1.0, 0.0), true).unwrap();
        assert_eq!(tf.lookup("base", "sensor", None).unwrap().1, 0);
        assert!(tf.lookup("base", "sensor", Some(7 * SECOND)).is_ok());
    }

    #[test]
    fn a_lookup_after_the_buffer_is_an_error() {
        let mut tf = buffer();
        tf.insert("world", "base", 10 * SECOND, at(0.0, 0.0, 0.0), false).unwrap();
        assert_eq!(tf.lookup("world", "base", Some(11 * SECOND)), Err(TfError::AfterBuffer));
    }

    #[test]
    fn a_lookup_before_the_buffer_is_an_error() {
        let mut tf = TfBuffer::new(std::time::Duration::from_secs(1));
        for second in 0..=5 {
            tf.insert("world", "base", second * SECOND, at(0.0, 0.0, 0.0), false).unwrap();
        }
        assert_eq!(
            tf.lookup("world", "base", Some(3 * SECOND + SECOND / 2)),
            Err(TfError::BeforeBuffer)
        );
        assert!(tf.lookup("world", "base", Some(4 * SECOND + SECOND / 2)).is_ok());
    }

    #[test]
    fn unconnected_frames_are_an_error() {
        let mut tf = buffer();
        tf.insert("world", "a", 0, at(1.0, 0.0, 0.0), true).unwrap();
        tf.insert("map", "b", 0, at(1.0, 0.0, 0.0), true).unwrap();
        assert_eq!(tf.lookup("a", "b", None), Err(TfError::NotConnected));
        assert_eq!(tf.lookup("a", "b", Some(SECOND)), Err(TfError::NotConnected));
    }

    #[test]
    fn unconnected_dynamic_frames_are_an_error_at_any_time() {
        // The crate reports a sampling failure before a disconnection; the buffer must not.
        let mut tf = buffer();
        tf.insert("world", "a", SECOND, at(1.0, 0.0, 0.0), false).unwrap();
        tf.insert("map", "b", SECOND, at(1.0, 0.0, 0.0), false).unwrap();
        assert_eq!(tf.lookup("a", "b", Some(5 * SECOND)), Err(TfError::NotConnected));
        assert_eq!(
            tf.lookup("a", "nowhere", Some(5 * SECOND)),
            Err(TfError::UnknownFrame("nowhere".into()))
        );
    }

    #[test]
    fn an_unknown_frame_is_named() {
        let mut tf = buffer();
        tf.insert("world", "base", 0, at(1.0, 0.0, 0.0), true).unwrap();
        assert_eq!(
            tf.lookup("world", "nowhere", None),
            Err(TfError::UnknownFrame("nowhere".into()))
        );
        assert_eq!(
            tf.lookup("/nowhere", "base", Some(SECOND)),
            Err(TfError::UnknownFrame("nowhere".into()))
        );
    }

    #[test]
    fn an_empty_frame_is_refused() {
        let mut tf = buffer();
        assert_eq!(
            tf.insert("", "a", 0, at(0.0, 0.0, 0.0), false),
            Err(TfError::InvalidFrame(String::new()))
        );
        assert_eq!(
            tf.insert("a", "/", 0, at(0.0, 0.0, 0.0), false),
            Err(TfError::InvalidFrame("/".into()))
        );
        assert_eq!(tf.lookup("", "a", None), Err(TfError::InvalidFrame(String::new())));
    }

    #[test]
    fn a_leading_slash_is_stripped() {
        let mut tf = buffer();
        tf.insert("/world", "/base", SECOND, at(1.0, 0.0, 0.0), false).unwrap();
        assert!(tf.lookup("world", "base", Some(SECOND)).is_ok());
        assert!(tf.lookup("/world", "base", Some(SECOND)).is_ok());
    }

    #[test]
    fn an_invalid_transform_is_rejected() {
        let mut tf = buffer();
        let skewed = Rigid { translation: [0.0; 3], rotation_xyzw: [0.0, 0.0, 0.0, 2.0] };
        assert!(matches!(tf.insert("a", "b", 0, skewed, false), Err(TfError::Rejected(_))));
        tf.insert("a", "b", SECOND, at(0.0, 0.0, 0.0), false).unwrap();
        // A frame keeps its parent: a second parent is refused, not silently re-hung.
        assert!(matches!(
            tf.insert("c", "b", SECOND, at(0.0, 0.0, 0.0), false),
            Err(TfError::Rejected(_))
        ));
    }

    #[test]
    fn a_non_finite_translation_or_rotation_is_rejected() {
        let mut tf = buffer();
        let nan = f64::NAN;
        let moved = Rigid { translation: [nan, 0.0, 0.0], rotation_xyzw: IDENTITY };
        assert!(matches!(
            tf.insert("map", "base", SECOND, moved, false),
            Err(TfError::Rejected(_))
        ));
        let turned = Rigid { translation: [0.0; 3], rotation_xyzw: [0.0, 0.0, f64::INFINITY, 1.0] };
        assert!(matches!(
            tf.insert("map", "base", SECOND, turned, false),
            Err(TfError::Rejected(_))
        ));
        assert!(matches!(tf.lookup("map", "base", None), Err(TfError::UnknownFrame(_))));
        // The sender's check gives the same answer as the buffer.
        assert!(matches!(check("map", "base", SECOND, moved, false), Err(TfError::Rejected(_))));
        assert!(matches!(
            check("map", "base", -1, at(0.0, 0.0, 0.0), false),
            Err(TfError::Rejected(_))
        ));
        assert!(matches!(
            check("", "base", SECOND, at(0.0, 0.0, 0.0), false),
            Err(TfError::InvalidFrame(_))
        ));
        assert_eq!(check("map", "base", SECOND, at(0.0, 0.0, 0.0), false), Ok(()));
        assert_eq!(
            check("map", "base", -1, at(0.0, 0.0, 0.0), true),
            Ok(()),
            "a static stamp is not read"
        );
    }

    #[test]
    fn a_transform_more_than_the_buffer_behind_drops_the_frames_history() {
        let mut tf = buffer();
        assert_eq!(
            tf.insert("map", "base", 100 * SECOND, at(1.0, 0.0, 0.0), false),
            Ok(Inserted::Kept)
        );
        // The host clock stepped back 20 s, more than the 10 s buffer.
        let back = tf.insert("map", "base", 80 * SECOND, at(2.0, 0.0, 0.0), false);
        assert_eq!(back, Ok(Inserted::DroppedHistory { newest_nanos: 100 * SECOND }));
        let (pose, stamp) = tf.lookup("map", "base", None).unwrap();
        assert_eq!(stamp, 80 * SECOND, "the latest pose is the new timeline's");
        assert_eq!(pose.translation, [2.0, 0.0, 0.0]);
    }

    #[test]
    fn a_transform_within_the_buffer_keeps_the_history() {
        let mut tf = buffer();
        assert_eq!(
            tf.insert("map", "base", 100 * SECOND, at(1.0, 0.0, 0.0), false),
            Ok(Inserted::Kept)
        );
        assert_eq!(
            tf.insert("map", "base", 95 * SECOND, at(2.0, 0.0, 0.0), false),
            Ok(Inserted::Kept)
        );
        let (_, stamp) = tf.lookup("map", "base", None).unwrap();
        assert_eq!(stamp, 100 * SECOND);
    }

    #[test]
    fn descendants_and_static_transforms_keep_theirs_when_a_frame_starts_again() {
        let mut tf = buffer();
        tf.insert("map", "base", 100 * SECOND, at(1.0, 0.0, 0.0), false).unwrap();
        tf.insert("base", "lidar", 0, at(0.0, 0.0, 1.0), true).unwrap();
        tf.insert("map", "base", 70 * SECOND, at(3.0, 0.0, 0.0), false).unwrap();
        let (pose, _) = tf.lookup("map", "lidar", None).unwrap();
        assert_eq!(pose.translation, [3.0, 0.0, 1.0], "the static child hangs on the new pose");
        // A static transform is never dropped by the rule.
        assert_eq!(tf.insert("map", "fixed", 0, at(0.0, 0.0, 0.0), true), Ok(Inserted::Kept));
        assert_eq!(tf.insert("map", "fixed", 0, at(0.0, 0.0, 0.0), true), Ok(Inserted::Kept));
    }
}
