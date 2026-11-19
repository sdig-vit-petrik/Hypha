//! Echo suppression for automatic same-name bridging.

#![allow(clippy::expect_used)]

use hypha_ros_bridge::automatic::{RecentSample, RecentSamples};

fn sample(topic: &str, stamp_nanos: u64, bytes: &[u8]) -> RecentSample {
    RecentSample { topic: topic.into(), stamp_nanos, bytes: bytes.to_vec() }
}

#[test]
fn a_republished_frame_is_recognised_from_what_the_wire_reports() {
    let mut recent = RecentSamples::new(8);
    recent.remember(sample("/legacy/text", 42, &[1, 2, 3]));

    assert!(recent.take("/legacy/text", 42, &[1, 2, 3]));
    assert!(recent.is_empty(), "a matched frame must be released");
}

#[test]
fn a_matched_frame_is_taken_only_once() {
    let mut recent = RecentSamples::new(8);
    recent.remember(sample("/legacy/text", 42, &[1, 2, 3]));

    assert!(recent.take("/legacy/text", 42, &[1, 2, 3]));
    assert!(
        !recent.take("/legacy/text", 42, &[1, 2, 3]),
        "a second identical frame is new traffic, not the same echo"
    );
}

#[test]
fn a_foreign_frame_is_never_suppressed() {
    let mut recent = RecentSamples::new(8);
    recent.remember(sample("/legacy/text", 42, &[1, 2, 3]));

    assert!(!recent.take("/other/topic", 42, &[1, 2, 3]), "a different topic must pass");
    assert!(!recent.take("/legacy/text", 43, &[1, 2, 3]), "a different timestamp must pass");
    assert!(!recent.take("/legacy/text", 42, &[9, 9, 9]), "a different payload must pass");
    assert_eq!(recent.len(), 1, "no failed match may consume the entry");
}

#[test]
fn the_filter_never_grows_past_its_limit() {
    let mut recent = RecentSamples::new(4);
    for index in 0..16_u64 {
        recent.remember(sample("/legacy/text", index, &[1]));
    }

    assert_eq!(recent.len(), 4);
    assert!(!recent.take("/legacy/text", 0, &[1]), "the oldest entries are replaced");
    assert!(recent.take("/legacy/text", 15, &[1]), "the newest entry is kept");
}

#[test]
fn a_matched_frame_releases_its_payload() {
    let mut recent = RecentSamples::new(256);
    for index in 0..64_u64 {
        recent.remember(sample("/legacy/text", index, &[7; 32]));
    }
    for index in 0..64_u64 {
        assert!(recent.take("/legacy/text", index, &[7; 32]));
    }

    assert!(recent.is_empty(), "matched frames must not stay held after the echo arrives");
}
