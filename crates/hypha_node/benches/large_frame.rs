//! The two ends of a large frame over the real transport, in one process and one thread.
//!
//! - publish: copy the payload into a slot, seal it (the integrity code) and send it;
//! - receive: take the sample and verify it (the integrity code again), no copy.
//!
//! Neither call waits, so the time of each is the CPU time of its thread. Sizes 5 MiB
//! (an image) and 15 MiB (a large image). Needs writable `/dev/shm` and `/tmp/iceoryx2`.
//! Run with `cargo bench -p hypha_node --bench large_frame`. Not a CI gate: see
//! `docs/engineering-traps.md`, "Benchmarks".
#![expect(clippy::expect_used, reason = "a refused setup or a lost sample stops the run")]

use std::time::{Duration, Instant};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use hypha_node::sdk::{BusNode, PublisherOptions, RawPublisher, RawSubscriber, SubscriberOptions};
use hypha_testkit::unique_name;

const DATA_TYPE: &str = "hypha/LargeFrame";
const MD5: &str = "0123456789abcdef0123456789abcdef";
const MIB: usize = 1024 * 1024;

fn send(publisher: &mut RawPublisher, payload: &[u8]) {
    assert!(publisher.publish(payload), "the transport refused the frame");
}

fn take(subscriber: &mut RawSubscriber, size: usize) {
    let sample = subscriber.try_receive().expect("a verified frame").expect("the frame sent");
    assert_eq!(sample.bytes().map(<[u8]>::len), Some(size));
}

fn large_frame(c: &mut Criterion) {
    let node = BusNode::new(&unique_name("bench_large_frame")).expect("node");
    for (size, name) in [(5 * MIB, "5MiB"), (15 * MIB, "15MiB")] {
        let topic = format!("/{}", unique_name("bench_large_frame"));
        let options = PublisherOptions::default().bounded(size);
        let mut publisher =
            node.publish_raw_configured(&topic, DATA_TYPE, MD5, "", options).expect("publisher");
        let options = SubscriberOptions::default().keeping(2);
        let mut subscriber = node.subscribe_raw_with_options(&topic, options).expect("subscriber");
        let payload: Vec<u8> = (0..size).map(|index| index.to_le_bytes()[0] ^ 0x5A).collect();

        let mut group = c.benchmark_group(format!("large_frame/{name}"));
        group.sample_size(20);
        group.throughput(Throughput::Bytes(u64::try_from(size).expect("a size fits u64")));
        group.bench_function("publish", |b| {
            b.iter_custom(|frames| {
                let mut spent = Duration::ZERO;
                for _ in 0..frames {
                    let started = Instant::now();
                    send(&mut publisher, &payload);
                    spent = spent.saturating_add(started.elapsed());
                    take(&mut subscriber, size);
                }
                spent
            });
        });
        group.bench_function("receive", |b| {
            b.iter_custom(|frames| {
                let mut spent = Duration::ZERO;
                for _ in 0..frames {
                    send(&mut publisher, &payload);
                    let started = Instant::now();
                    take(&mut subscriber, size);
                    spent = spent.saturating_add(started.elapsed());
                }
                spent
            });
        });
        group.finish();
    }
}

criterion_group!(benches, large_frame);
criterion_main!(benches);
