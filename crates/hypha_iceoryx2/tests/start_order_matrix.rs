//! Every creation order of publisher, subscriber and wildcard subscriber communicates.

#![allow(clippy::expect_used)]

mod support;
use support::sealed_send;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hypha_core::port::{QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::Iox2Transport;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn unique_prefix(label: &str) -> String {
    let pid = std::process::id();
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{label}_{pid}_{count}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointKind {
    Publisher,
    Subscriber,
    Wildcard,
}

const ORDERS: [[EndpointKind; 3]; 6] = [
    [EndpointKind::Publisher, EndpointKind::Subscriber, EndpointKind::Wildcard],
    [EndpointKind::Publisher, EndpointKind::Wildcard, EndpointKind::Subscriber],
    [EndpointKind::Subscriber, EndpointKind::Publisher, EndpointKind::Wildcard],
    [EndpointKind::Subscriber, EndpointKind::Wildcard, EndpointKind::Publisher],
    [EndpointKind::Wildcard, EndpointKind::Publisher, EndpointKind::Subscriber],
    [EndpointKind::Wildcard, EndpointKind::Subscriber, EndpointKind::Publisher],
];

#[test]
fn start_order_matrix_all_orders_succeed_and_communicate() {
    let transport = Iox2Transport::new()
        .expect("an iceoryx2 transport: /dev/shm and /tmp/iceoryx2 must be writable");

    for (i, order) in ORDERS.iter().enumerate() {
        let prefix = unique_prefix(&format!("matrix_{i}"));
        let topic = format!("/{prefix}/data");
        let pattern = format!("/{prefix}/*");

        let mut publisher = None;
        let mut subscriber = None;
        let mut wildcard = None;

        for kind in order {
            match kind {
                EndpointKind::Publisher => {
                    let publ = transport
                        .publish(&TopicSpec::new(&topic).with_type("std_msgs/String"))
                        .expect("publisher starts in this order");
                    publisher = Some(publ);
                }
                EndpointKind::Subscriber => {
                    let sub = transport
                        .subscribe(
                            &TopicSpec::new(&topic)
                                .with_type("std_msgs/String")
                                .keeping(2)
                                .with_qos(QosRequest::Declared(QosPolicy::new(
                                    "default", 8, 16, 32,
                                ))),
                        )
                        .expect("subscriber starts in this order");
                    subscriber = Some(sub);
                }
                EndpointKind::Wildcard => {
                    let wc = transport
                        .subscribe(
                            &TopicSpec::new(&pattern).with_type("std_msgs/String").keeping(2),
                        )
                        .expect("wildcard subscriber starts in this order");
                    wildcard = Some(wc);
                }
            }
        }

        let publisher = publisher.expect("publisher was created");
        let subscriber = subscriber.expect("subscriber was created");
        let wildcard = wildcard.expect("wildcard was created");

        let mut loan = publisher.loan(16).expect("publisher loans sample");
        loan.as_mut_bytes().fill(0x55);
        loan.send_frame(sealed_send::metadata(), 0).expect("publisher sends sample");

        let mut received_sub = false;
        for _ in 0..10 {
            if let Ok(Some(sample)) = subscriber.try_receive() {
                assert_eq!(sample.payload(), &[0x55; 16]);
                received_sub = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(received_sub, "order {order:?}: direct subscriber did not receive sample");

        // A wildcard created after the service discovered it at creation.
        let wildcard_created_after_service = order[0] != EndpointKind::Wildcard;
        if wildcard_created_after_service {
            let mut received_wc = false;
            for _ in 0..10 {
                if let Ok(Some(sample)) = wildcard.try_receive() {
                    assert_eq!(sample.payload(), &[0x55; 16]);
                    received_wc = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(received_wc, "order {order:?}: wildcard subscriber did not receive sample");
        }
    }
}
