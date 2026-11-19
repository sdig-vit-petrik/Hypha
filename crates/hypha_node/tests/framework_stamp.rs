//! The framework stamp of a frame follows the framework clock when nothing overrides it.
#![allow(clippy::expect_used)]

use std::time::{SystemTime, UNIX_EPOCH};

use hypha_node::sdk::{BusNode, SubscriberOptions};

/// The wall clock in nanoseconds, read the way the framework reads it.
fn wall_nanos() -> u64 {
    u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).expect("UTC").as_nanos())
        .expect("wall time fits u64")
}

#[test]
fn an_unoverridden_frame_is_stamped_with_the_wall_clock() {
    let bus = BusNode::new("framework_stamp_wall").expect("node");
    let nonce = wall_nanos();
    let topic = format!("/framework_stamp_wall_{}_{nonce:x}", std::process::id());
    let mut publisher = bus
        .publish_raw(
            &topic,
            "std_msgs/UInt8",
            &hypha_node::sdk::md5::md5_from_definition("std_msgs/UInt8", "uint8 data\n")
                .expect("md5"),
            "uint8 data\n",
        )
        .expect("publisher");

    let before = wall_nanos();
    let mut subscriber = bus
        .subscribe_raw_with_schema(
            &topic,
            "std_msgs/UInt8",
            "",
            SubscriberOptions::default().keeping(4),
        )
        .expect("subscription");
    assert!(publisher.publish(&[1]));
    let after = wall_nanos();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let identity = loop {
        if let Some(sample) = subscriber.try_receive().expect("receive") {
            break sample.to_raw_sample().expect("identity");
        }
        assert!(std::time::Instant::now() < deadline, "no frame arrived");
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert!(
        (before..=after).contains(&identity.framework_sent_nanos),
        "framework stamp {} is outside [{before}, {after}]",
        identity.framework_sent_nanos
    );
}
