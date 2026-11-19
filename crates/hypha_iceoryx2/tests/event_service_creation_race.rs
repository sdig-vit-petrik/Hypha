//! Two endpoints reaching one event service while a third tears it down.
#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hypha_core::port::TopicSpec;
use hypha_iceoryx2::Iox2Transport;
use hypha_testkit::unique_name as unique;

/// Advertising and dropping a schema-bearing topic from several threads at once.
#[test]
fn concurrent_endpoint_churn_on_one_event_service_never_reports_corruption() {
    let topic = format!("/{}", unique("topic"));
    let failures = Arc::new(AtomicU64::new(0));
    let attempts = Arc::new(AtomicU64::new(0));

    let mut workers = Vec::new();
    for worker in 0..4_u32 {
        let topic = topic.clone();
        let failures = Arc::clone(&failures);
        let attempts = Arc::clone(&attempts);
        workers.push(std::thread::spawn(move || {
            for round in 0..12_u32 {
                let name = format!("{}_{worker}_{round}", unique("node"));
                let Ok(transport) = Iox2Transport::with_name(&name) else {
                    // A node this machine cannot create is an environment limit.
                    continue;
                };
                let spec = TopicSpec::new(&topic)
                    .with_type("std_msgs/String")
                    .with_md5("992ce8a1687cec8c8bd883ec73ca41d1")
                    .with_definition("string data\n");
                attempts.fetch_add(1, Ordering::SeqCst);
                if let Err(cause) = transport.publish(&spec) {
                    eprintln!("hypha_iceoryx2 test: an endpoint was refused: {cause:?}");
                    failures.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
    }
    for worker in workers {
        worker.join().expect("worker thread");
    }

    let refused = failures.load(Ordering::SeqCst);
    let tried = attempts.load(Ordering::SeqCst);
    assert!(tried > 0, "the test performed no attempts");
    // The retry has to absorb every state that clears by itself.
    assert_eq!(refused, 0, "{refused} of {tried} endpoints were refused by contention");
}
