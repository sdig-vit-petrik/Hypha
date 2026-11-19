//! Notification integration tests.

mod support;
use support::sealed_send;

use core::time::Duration;

use hypha_iceoryx2::Iox2Transport;

use hypha_core::port::TopicSpec;
use hypha_testkit::unique_name;

#[test]
// These tests use expect and direct indexing to identify failed test setup precisely.
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn notification_wakes_another_transport_and_leaves_the_sample_queued() {
    let topic = format!("/{}", unique_name("notification_round_trip"));
    let publisher_transport =
        Iox2Transport::with_name(&unique_name("notification_publisher")).expect("publisher node");
    let subscriber_transport =
        Iox2Transport::with_name(&unique_name("notification_subscriber")).expect("subscriber node");
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data");
    let publisher = publisher_transport.publish(&spec).expect("publish");
    let subscriber = subscriber_transport.subscribe(&spec).expect("subscribe");
    // A `WaitSet` opens its listener when the subscription attaches, before data flows.
    let listener = subscriber
        .open_listener()
        .expect("open a listener")
        .expect("a literal topic has an event service");

    let mut loan = publisher.loan(4).expect("loan");
    loan.as_mut_bytes().copy_from_slice(b"scan");
    loan.send_frame(sealed_send::metadata(), 0).expect("send");

    let fired = listener.timed_wait(|_| {}, Duration::from_secs(1)).expect("wait");
    assert!(fired > 0, "the send must wake the listener");
    let sample = subscriber.try_receive().expect("receive").expect("queued sample");
    assert_eq!(sample.payload(), b"scan");
}

#[test]
// These tests use expect and direct indexing to identify failed test setup precisely.
#[allow(clippy::expect_used, clippy::indexing_slicing)]
fn a_latched_replay_wakes_a_subscriber_that_joined_late() {
    let topic = format!("/{}", unique_name("notification_latched"));
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data").keeping(1).latched();
    let publisher_transport =
        Iox2Transport::with_name(&unique_name("latched_publisher")).expect("publisher node");
    let publisher = publisher_transport.publish(&spec).expect("publish");

    let mut loan = publisher.loan(4).expect("loan");
    loan.as_mut_bytes().copy_from_slice(b"held");
    loan.send_frame(sealed_send::metadata(), 0).expect("send");

    // The subscriber attaches after the send, so only the replay delivers; it must carry a wake.
    let subscriber_transport =
        Iox2Transport::with_name(&unique_name("latched_subscriber")).expect("subscriber node");
    let subscriber = subscriber_transport.subscribe(&spec).expect("subscribe");
    let listener = subscriber
        .open_listener()
        .expect("open a listener")
        .expect("a notified subscriber has a listener");

    publisher_transport.replay_latched();
    let fired = listener.timed_wait(|_| {}, Duration::from_secs(2)).expect("wait");
    assert!(fired > 0, "the replay must wake the listener");
    let sample = subscriber.try_receive().expect("receive").expect("replayed sample");
    assert_eq!(sample.payload(), b"held");
}

#[test]
#[allow(clippy::expect_used)]
fn a_wildcard_subscription_has_no_listener() {
    let transport =
        Iox2Transport::with_name(&unique_name("notification_wildcard")).expect("transport");
    let prefix = unique_name("notification_pattern");
    let topic = TopicSpec::new(&format!("/{prefix}/a")).with_type("hypha_test/Data");
    let _publisher = transport.publish(&topic).expect("publish");

    // A pattern has no one event service to listen on: the subscription is polled.
    let pattern = TopicSpec::new(&format!("/{prefix}/*")).with_type("hypha_test/Data");
    let subscriber = transport.subscribe(&pattern).expect("subscribe");
    assert!(subscriber.open_listener().expect("open a listener").is_none());
}
