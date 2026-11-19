//! An undeclared subscriber never takes a slot a declared subscriber is entitled to.

#![allow(clippy::expect_used)]

use hypha_core::port::{PortError, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::Iox2Transport;
use hypha_iceoryx2::qos::RESERVED_OBSERVER_SUBSCRIBERS;
use hypha_testkit::unique_name as unique;

#[test]
#[allow(clippy::expect_used)]
fn an_undeclared_subscriber_never_takes_a_declared_subscribers_slot() {
    const DECLARED_SUBSCRIBERS: usize = 2;
    let transport = Iox2Transport::with_name(&unique("observer_slots")).expect("transport");
    let topic = format!("/{}", unique("observer_slots_topic"));
    let policy = QosPolicy::new("observer_test", 8, DECLARED_SUBSCRIBERS, 8);
    let declared = TopicSpec::new(&topic)
        .with_type("hypha_test/Observer")
        .keeping(2)
        .with_qos(QosRequest::Declared(policy));
    let undeclared = TopicSpec::new(&topic).with_type("hypha_test/Observer").keeping(2);
    let _publisher = transport.publish(&declared).expect("publisher");

    // The observers take their reserved slots, and no more.
    let observers: Vec<_> = (0..RESERVED_OBSERVER_SUBSCRIBERS)
        .map(|_| transport.subscribe(&undeclared).expect("an observer within its reserved slots"))
        .collect();
    let refused = transport.subscribe(&undeclared).err();
    assert_eq!(refused, Some(PortError::ResourceLimit), "the observer slots are full");

    // The declared subscribers still get every slot of their policy.
    let declared_subscribers: Vec<_> = (0..DECLARED_SUBSCRIBERS)
        .map(|_| transport.subscribe(&declared).expect("a declared subscriber within its policy"))
        .collect();
    let over_policy = transport.subscribe(&declared).err();
    assert_eq!(over_policy, Some(PortError::ResourceLimit), "the policy's count is the limit");

    drop(declared_subscribers);
    drop(observers);
}
