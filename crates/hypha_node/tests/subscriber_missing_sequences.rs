//! The SDK view of the missing-sequence count for a raw subscriber.

use hypha_node::sdk::{BusNode, SubscriberOptions};
use hypha_testkit::unique_name;

#[test]
#[allow(clippy::expect_used)]
fn steady_traffic_reports_no_missing_sequence() {
    let bus = BusNode::new(&unique_name("missing_steady_rust")).expect("node");
    let topic = format!("/{}", unique_name("missing_steady"));
    let mut publisher = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let mut subscriber = bus
        .subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(1))
        .expect("subscriber");
    assert_eq!(subscriber.missing_sequences().expect("count"), 0);

    for value in 0_u8..4 {
        assert!(publisher.publish(&[value]));
        assert!(subscriber.try_receive().expect("receive").is_some());
    }
    assert_eq!(subscriber.missing_sequences().expect("count"), 0);
}

#[test]
#[allow(clippy::expect_used)]
fn an_overflowing_queue_reports_the_samples_it_dropped() {
    let bus = BusNode::new(&unique_name("missing_overflow_rust")).expect("node");
    let topic = format!("/{}", unique_name("missing_overflow"));
    let mut publisher = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let mut subscriber = bus
        .subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(1))
        .expect("subscriber");

    // The first accepted frame is the baseline the overflow is measured from.
    assert!(publisher.publish(&[0]));
    assert!(subscriber.try_receive().expect("receive").is_some());
    for value in 1_u8..=5 {
        assert!(publisher.publish(&[value]));
    }
    assert!(subscriber.try_receive().expect("receive").is_some(), "the newest frame survives");

    let missing = subscriber.missing_sequences().expect("count");
    assert!(missing >= 4, "four frames were dropped, the count reports {missing}");
}

#[test]
#[allow(clippy::expect_used)]
fn a_publisher_that_came_after_the_subscriber_owes_it_its_first_frames() {
    // #247: the publisher's port did not exist when the subscriber was created, so its first
    // frames were owed to the subscriber; the queue of one pushed out the first two. A raw
    // subscriber needs the topic's stream: a first publisher, which sends nothing, opens it.
    let bus = BusNode::new(&unique_name("missing_after_rust")).expect("node");
    let topic = format!("/{}", unique_name("missing_after"));
    let _opens_the_stream = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let mut subscriber = bus
        .subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(1))
        .expect("subscriber");
    let mut publisher = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    for value in 0_u8..3 {
        assert!(publisher.publish(&[value]));
    }
    assert!(subscriber.try_receive().expect("receive").is_some(), "the newest frame survives");
    assert_eq!(subscriber.missing_sequences().expect("count"), 2);
    assert_eq!(subscriber.sent_before_first(), 0, "nothing was sent before the subscriber");
}

#[test]
#[allow(clippy::expect_used)]
fn a_late_joiner_does_not_count_what_was_sent_before_it_as_lost() {
    let bus = BusNode::new(&unique_name("missing_late_rust")).expect("node");
    let topic = format!("/{}", unique_name("missing_late"));
    let mut publisher = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    for value in 0_u8..3 {
        assert!(publisher.publish(&[value]));
    }
    let mut subscriber = bus
        .subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(4))
        .expect("subscriber");
    for value in 3_u8..6 {
        assert!(publisher.publish(&[value]));
    }
    let mut received = 0;
    while subscriber.try_receive().expect("receive").is_some() {
        received += 1;
    }
    assert_eq!(received, 3, "the frames sent before the subscriber are history");
    assert_eq!(subscriber.missing_sequences().expect("count"), 0);
    assert_eq!(subscriber.sent_before_first(), 3, "the late joiner's evidence");
}

#[test]
#[allow(clippy::expect_used)]
fn a_loss_of_the_first_frames_of_a_publisher_that_existed_before_is_not_counted() {
    // The limit (communication-integrity.md §5): the publisher existed when the subscriber was
    // created, so its first frame may have been sent before it, as for a late joiner.
    let bus = BusNode::new(&unique_name("missing_before_rust")).expect("node");
    let topic = format!("/{}", unique_name("missing_before"));
    let mut publisher = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let mut subscriber = bus
        .subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(1))
        .expect("subscriber");
    for value in 0_u8..3 {
        assert!(publisher.publish(&[value]));
    }
    assert!(subscriber.try_receive().expect("receive").is_some());
    assert_eq!(subscriber.missing_sequences().expect("count"), 0);
    assert_eq!(subscriber.sent_before_first(), 2);
}
