//! A `NodeState` publisher's frame carries the compact hash of the message's MD5.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use hypha_node::sdk::BusNode;
use hypha_node::sdk::msgs::hypha_msgs::NodeState;
use std::time::{Duration, Instant};

const TOPIC: &str = "/identity_probe/state";
/// The longest the subscriber may take to receive the first frame.
const RECEIVE_LIMIT: Duration = Duration::from_secs(10);

#[test]
fn a_node_state_frame_carries_the_hash_of_its_md5() {
    let publisher_bus = BusNode::new("identity_publisher").expect("the publisher's bus");
    let listener_bus = BusNode::new("identity_listener").expect("the listener's bus");
    let mut publisher = publisher_bus
        .publish_raw(TOPIC, NodeState::TYPE_NAME, NodeState::SCHEMA_HASH, "")
        .expect("a NodeState publisher");
    let mut subscriber = listener_bus.subscribe_raw(TOPIC, 4).expect("a subscriber");
    let expected =
        hypha_node::sdk::schema_hash_from_md5(NodeState::SCHEMA_HASH).expect("a valid MD5");

    let began = Instant::now();
    loop {
        assert!(publisher.publish(&[1, 2, 3]), "a frame on {TOPIC}");
        if let Some(sample) = subscriber.try_receive().expect("the receive") {
            let identity = sample.identity().expect("the frame's identity");
            assert_eq!(identity.schema_hash, expected, "the frame carries the MD5's hash");
            return;
        }
        assert!(began.elapsed() < RECEIVE_LIMIT, "no frame arrived on {TOPIC}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
