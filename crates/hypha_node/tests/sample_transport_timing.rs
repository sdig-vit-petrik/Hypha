//! Transport timing read back from a received sample.
#![allow(clippy::expect_used)]

use hypha_node::sdk::{BusNode, SubscriberOptions};
use hypha_testkit::unique_name;

#[test]
fn a_received_sample_reports_the_instant_the_transport_sent_it() {
    let bus = BusNode::new(&unique_name("node")).expect("node");
    let topic = format!("/hypha_node/sample_time/{}", unique_name("topic"));
    let mut publisher = bus
        .publish_raw(&topic, "std_msgs/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let options = SubscriberOptions::default().keeping(2);
    let mut subscriber = bus.subscribe_raw_with_options(&topic, options).expect("subscriber");
    assert!(publisher.publish(b"timing"));

    let sample = subscriber.try_receive().expect("receive");
    let sample = sample.expect("the published frame arrived");

    let identity = sample.identity().expect("identity");
    assert!(identity.transport_sent_nanos > 0, "a sent frame carries a send time");
}
