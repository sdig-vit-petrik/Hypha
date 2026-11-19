//! A wildcard subscription resolves its topics once, at creation.

mod support;
use support::sealed_send;

use std::time::{Duration, Instant};

use hypha_core::port::{PortError, QosRequest, TopicSpec};
use hypha_iceoryx2::{Iox2Publisher, Iox2Subscriber, Iox2Transport};
use hypha_testkit::unique_name;

#[allow(clippy::expect_used)]
fn send(publisher: &mut Iox2Publisher, payload: &[u8]) {
    let mut loan = publisher.loan(payload.len()).expect("loan");
    loan.as_mut_bytes().copy_from_slice(payload);
    loan.send_frame(sealed_send::metadata(), 0).expect("send");
}

/// Polls for up to `window`, sending on each pass; true when a sample arrived.
#[allow(clippy::expect_used)]
fn receives_within(
    subscriber: &mut Iox2Subscriber,
    publisher: &mut Iox2Publisher,
    window: Duration,
) -> bool {
    let deadline = Instant::now().checked_add(window).unwrap_or_else(Instant::now);
    while Instant::now() < deadline {
        send(publisher, b"hello");
        if subscriber.try_receive().expect("receive").is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
#[allow(clippy::expect_used)]
fn a_publisher_present_at_subscribe_time_is_followed() {
    let transport = Iox2Transport::with_name(&unique_name("oneshot_early")).expect("transport");
    let prefix = format!("/{}", unique_name("early"));
    let topic = format!("{prefix}/one");
    let mut publisher =
        transport.publish(&TopicSpec::new(&topic).with_type("std_msgs/String")).expect("publisher");

    let pattern = format!("{prefix}/*");
    let mut subscriber = transport
        .subscribe(&TopicSpec::new(&pattern).with_type("std_msgs/String").keeping(2))
        .expect("wildcard subscriber");

    assert!(
        receives_within(&mut subscriber, &mut publisher, Duration::from_secs(2)),
        "a topic that existed at subscribe time must be followed"
    );
    let sample = subscriber.try_receive().expect("receive");
    if let Some(sample) = sample {
        assert_eq!(sample.topic(), topic);
    }
}

#[test]
#[allow(clippy::expect_used)]
fn a_publisher_that_starts_later_is_not_followed() {
    let transport = Iox2Transport::with_name(&unique_name("oneshot_late")).expect("transport");
    let prefix = format!("/{}", unique_name("late"));
    let pattern = format!("{prefix}/*");

    // Nothing matches yet, so this subscription follows nothing at all.
    let mut subscriber = transport
        .subscribe(&TopicSpec::new(&pattern).with_type("std_msgs/String").keeping(2))
        .expect("wildcard subscriber");

    let topic = format!("{prefix}/one");
    let mut publisher =
        transport.publish(&TopicSpec::new(&topic).with_type("std_msgs/String")).expect("publisher");

    assert!(
        !receives_within(&mut subscriber, &mut publisher, Duration::from_secs(2)),
        "discovery runs once: a later publisher must not appear on its own"
    );

    // The caller follows it by subscribing to the resolved name instead.
    let mut named = transport
        .subscribe(&TopicSpec::new(&topic).with_type("std_msgs/String").keeping(2))
        .expect("named subscriber");
    assert!(
        receives_within(&mut named, &mut publisher, Duration::from_secs(2)),
        "a named subscription reaches the same publisher"
    );
}

#[test]
#[allow(clippy::expect_used)]
fn a_wildcard_refuses_a_matching_topic_with_insufficient_queue_depth() {
    let transport = Iox2Transport::with_name(&unique_name("oneshot_qos")).expect("transport");
    let prefix = format!("/{}", unique_name("qos"));
    let topic = format!("{prefix}/one");
    let _publisher =
        transport.publish(&TopicSpec::new(&topic).with_type("std_msgs/String")).expect("publisher");

    let pattern = format!("{prefix}/*");
    let result =
        transport.subscribe(&TopicSpec::new(&pattern).with_type("std_msgs/String").keeping(33));
    assert!(matches!(result, Err(PortError::ResourceLimit)));
}

#[test]
#[allow(clippy::expect_used)]
fn a_wildcard_reports_exhausted_subscriber_capacity() {
    let transport = Iox2Transport::with_name(&unique_name("oneshot_capacity")).expect("transport");
    let prefix = format!("/{}", unique_name("capacity"));
    let topic = format!("{prefix}/one");
    let spec = TopicSpec::new(&topic).with_type("std_msgs/String");
    let declared = TopicSpec::new(&topic)
        .with_type("std_msgs/String")
        .with_qos(QosRequest::Declared(hypha_iceoryx2::qos::default_policy()));
    let _publisher = transport.publish(&declared).expect("publisher");
    // The declared subscribers fill their policy, and the observers their reserved slots.
    let mut subscribers = Vec::new();
    for _ in 0..hypha_iceoryx2::qos::DEFAULT_MAX_SUBSCRIBERS {
        subscribers.push(transport.subscribe(&declared).expect("declared subscriber"));
    }
    for _ in 0..hypha_iceoryx2::qos::RESERVED_OBSERVER_SUBSCRIBERS {
        subscribers.push(transport.subscribe(&spec).expect("observer within its reserved slots"));
    }

    let pattern = format!("{prefix}/*");
    let wildcard = TopicSpec::new(&pattern).with_type("std_msgs/String");
    let result = transport.subscribe(&wildcard);
    assert!(matches!(result, Err(PortError::ResourceLimit)));
}
