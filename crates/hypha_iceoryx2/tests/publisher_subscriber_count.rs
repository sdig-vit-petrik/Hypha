//! A publisher reports how many subscribers are connected to its topic.

use hypha_core::port::TopicSpec;
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name;

#[test]
#[allow(clippy::expect_used)]
fn publisher_counts_the_subscribers_that_connect_and_leave() {
    let transport = Iox2Transport::with_name(&unique_name("sub_count_local")).expect("transport");
    let topic = format!("/{}", unique_name("sub_count_topic"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
    let publisher = transport.publish(&spec).expect("publisher");
    assert_eq!(publisher.subscriber_count(), Some(0));

    let subscriber = transport.subscribe(&spec).expect("subscriber");
    assert_eq!(publisher.subscriber_count(), Some(1));

    drop(subscriber);
    assert_eq!(publisher.subscriber_count(), Some(0));
}

#[test]
#[allow(clippy::expect_used)]
fn publisher_counts_subscribers_held_by_another_transport() {
    let publishing = Iox2Transport::with_name(&unique_name("sub_count_pub")).expect("transport");
    let listening = Iox2Transport::with_name(&unique_name("sub_count_sub")).expect("transport");
    let topic = format!("/{}", unique_name("sub_count_remote"));
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2);
    let publisher = publishing.publish(&spec).expect("publisher");

    let first = listening.subscribe(&spec).expect("first subscriber");
    let second = listening.subscribe(&spec).expect("second subscriber");
    assert_eq!(publisher.subscriber_count(), Some(2));

    drop(first);
    assert_eq!(publisher.subscriber_count(), Some(1));
    drop(second);
    assert_eq!(publisher.subscriber_count(), Some(0));
}
