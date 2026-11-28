//! A subscriber's counter handle reads its counts after the subscriber moved into a
//! `WaitSet` attachment.

use std::time::Duration;

use hypha_node::sdk::{BusNode, SubscriberOptions, WaitSet};
use hypha_testkit::unique_name;

/// The longest a waking run waits for a sample that is already queued.
const RUN_BOUND: Duration = Duration::from_secs(10);

#[test]
#[expect(clippy::expect_used, reason = "a test fails on the first refused step")]
fn the_missing_sequence_count_reads_after_the_subscriber_was_attached_to_a_waitset() {
    let bus = BusNode::new(&unique_name("counters_rust")).expect("node");
    let topic = format!("/{}", unique_name("counters"));
    let mut publisher = bus
        .publish_raw(&topic, "test/Bytes", "d9d213e8c146065ec958333c617eb1a6", "")
        .expect("publisher");
    let subscriber = bus
        .subscribe_raw_with_options(&topic, SubscriberOptions::default().keeping(1))
        .expect("subscriber");
    let counters = subscriber.counters();
    let mut waitset = WaitSet::new(&bus).expect("waitset");
    waitset.attach_waking_subscription(subscriber, |_| Ok(())).expect("attach");

    // The first accepted frame is the baseline the overflow is measured from.
    assert!(publisher.publish(&[0]));
    assert!(waitset.run_for(RUN_BOUND).ok());
    assert_eq!(counters.missing_sequences(), 0);

    for value in 1_u8..=5 {
        assert!(publisher.publish(&[value]));
    }
    assert!(waitset.run_for(RUN_BOUND).ok());
    // A queue of one keeps the newest of the five: frames 1 to 4 are dropped.
    assert_eq!(counters.missing_sequences(), 4);
    // A dropped frame is a missing sequence, not a refused one: no frame came out of order.
    let refused = counters.metrics().into_iter().find(|(name, _)| name == "integrity.sequence_gap");
    assert_eq!(refused, Some(("integrity.sequence_gap".to_owned(), 0)));
}
