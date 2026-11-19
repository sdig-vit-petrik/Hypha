//! Owning receive loan budget: exhaustion, refusal, and release-then-retry.
#![allow(clippy::expect_used, reason = "a failed setup or receive is a test failure")]

use hypha_node::HyphaStatus;
use hypha_node::sdk::{BusNode, RawSubscriber, ReceiveError, SubscriberOptions};
use hypha_testkit::unique_name;

const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";

/// The value of the named metric of `subscriber`, or `None` when it does not emit that name.
fn metric(subscriber: &RawSubscriber, name: &str) -> Option<u64> {
    let list = subscriber.metrics().expect("metrics");
    list.into_iter().find(|(metric, _)| metric == name).map(|(_, value)| value)
}

#[test]
fn exhausting_the_loan_budget_refuses_until_a_sample_is_released() {
    let bus = BusNode::new(&unique_name("loan_budget_node")).expect("node");
    let topic = format!("/hypha_node/loan_budget/{}", unique_name("topic"));
    let mut publisher = bus.publish_raw(&topic, "std_msgs/Bytes", MD5, "").expect("publisher");
    // A depth of four is over the transport's own borrow cap of two, so the cap is the
    // budget that refuses the third loan.
    let options = SubscriberOptions::default().keeping(4);
    let mut subscriber = bus.subscribe_raw_with_options(&topic, options).expect("subscriber");

    for _ in 0_u8..3 {
        assert!(publisher.publish(&[0; 4]));
    }

    let first = subscriber.try_receive().expect("first receive");
    assert!(first.is_some());

    let second = subscriber.try_receive().expect("second receive");
    assert!(second.is_some());

    let third = subscriber.try_receive();
    let refused = third.err().map(ReceiveError::status);
    assert_eq!(refused, Some(HyphaStatus::BorrowLimit as i32));

    assert_eq!(metric(&subscriber, "loan.budget"), Some(2));
    assert_eq!(metric(&subscriber, "loan.held"), Some(2));
    assert!(metric(&subscriber, "loan.refused").is_some_and(|refused| refused > 0));

    drop(first);

    let retried = subscriber.try_receive().expect("retried receive");
    assert!(retried.is_some());

    drop(retried);
    drop(second);
}
