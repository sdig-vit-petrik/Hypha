//! A subscription with a malformed wildcard pattern is refused at creation.
#![allow(clippy::expect_used)]

use hypha_core::port::{PortError, TopicSpec};
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name;

#[test]
fn a_wildcard_pattern_with_a_star_inside_a_segment_is_refused() {
    let transport = Iox2Transport::with_name(&unique_name("pattern_star")).expect("transport");
    for pattern in ["/a/b*", "/a/*b", "/a/**x"] {
        let refusal =
            transport.subscribe(&TopicSpec::new(pattern).with_type("hypha_test/Data")).err();
        assert_eq!(refusal, Some(PortError::InvalidTopic), "{pattern:?} must be refused");
    }
}

#[test]
fn a_malformed_pattern_is_refused_before_the_notification_check() {
    let transport = Iox2Transport::with_name(&unique_name("pattern_notify")).expect("transport");
    let spec = TopicSpec::new("/a/b*").with_type("hypha_test/Data");
    assert_eq!(transport.subscribe(&spec).err(), Some(PortError::InvalidTopic));
}

#[test]
fn a_well_formed_wildcard_pattern_is_still_accepted() {
    let transport = Iox2Transport::with_name(&unique_name("pattern_ok")).expect("transport");
    for pattern in ["/a/*", "/a/**", "**"] {
        // "**" matches the shared bus, so another test's service can vanish mid-open; only a
        // refusal as malformed is a verdict.
        let refusal =
            transport.subscribe(&TopicSpec::new(pattern).with_type("hypha_test/Data")).err();
        assert_ne!(refusal, Some(PortError::InvalidTopic), "{pattern:?} must not be refused");
    }
}
