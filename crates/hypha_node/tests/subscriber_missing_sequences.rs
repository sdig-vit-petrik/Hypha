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

#[test]
#[allow(clippy::expect_used)]
fn a_subscriber_and_a_publisher_created_at_once_count_no_loss() {
    // #247, the same-instant race: a subscriber and a publisher created on two threads at
    // once. The publisher sends every 2 ms until the subscriber exists and two more after;
    // the queue holds what it sends meanwhile. It stays until the subscriber has read: a
    // publisher that leaves takes its unread frames with it. Whatever order the registry saw the
    // two ports in, nothing is lost: what the subscriber did not receive was sent before it.
    use std::sync::atomic::{AtomicBool, Ordering};
    const ROUNDS: usize = 60;
    let subscribing = BusNode::new(&unique_name("race_sub_rust")).expect("node");
    let publishing = BusNode::new(&unique_name("race_pub_rust")).expect("node");
    let topic = format!("/{}", unique_name("race"));
    let _opens_the_stream = publishing
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let (mut with_frames, mut came_after) = (0_usize, 0_usize);
    for round in 0..ROUNDS {
        // Each round starts the publisher later, from at once to past the subscriber's creation.
        let delay =
            std::time::Duration::from_micros(u64::try_from(round % 30).expect("small") * 5_000);
        let subscribed = AtomicBool::new(false);
        let start = std::sync::Barrier::new(2);
        let (mut reader, (publisher, sent)) = std::thread::scope(|scope| {
            let reading = scope.spawn(|| {
                start.wait();
                let port = subscribing
                    .subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(32))
                    .expect("subscriber");
                subscribed.store(true, Ordering::Release);
                port
            });
            let publisher = scope.spawn(|| {
                start.wait();
                std::thread::sleep(delay);
                let mut publisher = publishing
                    .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
                    .expect("publisher");
                let mut sent = 0_u64;
                let mut after = 0;
                while after < 2 {
                    if subscribed.load(Ordering::Acquire) {
                        after += 1;
                    }
                    assert!(publisher.publish(&[0]));
                    sent += 1;
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                (publisher, sent)
            });
            (reading.join().expect("subscriber thread"), publisher.join().expect("thread"))
        });
        let mut received = 0_u64;
        while reader.try_receive().expect("receive").is_some() {
            received += 1;
        }
        drop(publisher);
        let before = reader.sent_before_first();
        assert_eq!(reader.missing_sequences().expect("count"), 0, "{received} of {sent}");
        assert_eq!(received + before, sent, "{received} received, {before} before, of {sent}");
        with_frames += usize::from(received > 0);
        came_after += usize::from(received == sent);
    }
    eprintln!("{ROUNDS} rounds: {with_frames} received frames, {came_after} received every frame");
}

#[test]
#[allow(clippy::expect_used)]
fn a_latched_publisher_that_came_after_the_subscriber_owes_it_its_first_frames() {
    let bus = BusNode::new(&unique_name("missing_latch_rust")).expect("node");
    let topic = format!("/{}", unique_name("missing_latch"));
    let _opens_the_stream = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let mut subscriber = bus
        .subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(1))
        .expect("subscriber");
    let mut latched = bus
        .publish_raw_configured(
            &topic,
            "test/Bytes",
            "d9d213e8c146065ec958333c617eb1a6",
            "",
            hypha_node::sdk::PublisherOptions::default().latched(),
        )
        .expect("latched publisher");
    for value in 0_u8..3 {
        assert!(latched.publish(&[value]));
    }
    assert!(subscriber.try_receive().expect("receive").is_some());
    assert_eq!(subscriber.missing_sequences().expect("count"), 2);
}
