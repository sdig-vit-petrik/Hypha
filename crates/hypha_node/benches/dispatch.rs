//! Dispatch cost of one sample over the real transport, in one process.
//!
//! Each iteration publishes one 4-byte sample through the Rust SDK, runs the
//! `WaitSet` until the subscription callback has taken it, and returns: publish,
//! the iceoryx2 notification, one `WaitSet` turn and the callback. The
//! subscription is a waking one, so the run ends as soon as the sample is
//! delivered and no deadline is measured. The callback counts the deliveries and
//! the iteration checks that each run delivered exactly one.
//!
//! The cycle tick with one input and one output is measured through the C ABI, as
//! the cycle tests do: `cargo bench -p hypha_ffi --bench cycle`.
//!
//! Needs writable `/dev/shm` and `/tmp/iceoryx2`. Run with
//! `cargo bench -p hypha_node --bench dispatch`. Not a CI gate: see
//! `docs/engineering-traps.md`, "Benchmarks".
#![expect(clippy::expect_used, reason = "a refused setup or a lost sample stops the run")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use hypha_node::sdk::{BusNode, PublisherOptions, SubscriberOptions, WaitSet};
use hypha_testkit::unique_name;

const DATA_TYPE: &str = "std_msgs/Counter";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const DEFINITION: &str = "uint32 value\n";
/// The longest a run may wait for its one sample; the run ends at the delivery.
const RUN_LIMIT: Duration = Duration::from_secs(1);

fn publish_to_callback(c: &mut Criterion) {
    let node = BusNode::new(&unique_name("bench_dispatch")).expect("node");
    let topic = format!("/{}", unique_name("bench_dispatch"));
    let mut publisher = node
        .publish_raw_configured(&topic, DATA_TYPE, MD5, DEFINITION, PublisherOptions::default())
        .expect("publisher");
    let options = SubscriberOptions::default().keeping(8);
    let subscriber = node.subscribe_raw_with_options(&topic, options).expect("subscriber");
    let delivered = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&delivered);
    let mut waitset = WaitSet::new(&node).expect("waitset");
    waitset
        .attach_waking_subscription(subscriber, move |delivery| {
            delivery.payload.ok_or("unparsed frame")?;
            counter.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
        .expect("attach");

    let mut value: u32 = 0;
    c.bench_function("dispatch/publish_to_callback", |b| {
        b.iter(|| {
            value = value.wrapping_add(1);
            let before = delivered.load(Ordering::Relaxed);
            assert!(publisher.publish(&value.to_le_bytes()), "the transport refused the sample");
            let result = waitset.run_for(RUN_LIMIT);
            assert!(result.ok(), "{:?}", result.err());
            let after = delivered.load(Ordering::Relaxed);
            assert_eq!(after.wrapping_sub(before), 1, "one run delivers the one sample");
        });
    });
}

criterion_group!(benches, publish_to_callback);
criterion_main!(benches);
