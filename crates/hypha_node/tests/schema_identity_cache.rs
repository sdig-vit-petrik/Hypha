//! `SchemaIdentityCache`: retained-schema resolution shared by the tools and the ROS bridge.

#![allow(clippy::expect_used, clippy::panic)]

use std::time::{Duration, Instant};

use hypha_node::sdk::{BusNode, SCHEMA_RETRY_INTERVAL as RETRY_INTERVAL, SchemaIdentityCache};
use hypha_testkit::unique_name;

/// Publishes on a topic until the subscriber reports a frame, then returns its schema hash.
fn first_schema_hash(
    publisher: &mut hypha_node::sdk::RawPublisher,
    subscriber: &mut hypha_node::sdk::RawSubscriber,
) -> u64 {
    for _ in 0..100 {
        assert!(publisher.publish(&[1, 2, 3, 4]));
        if let Ok(Some(sample)) = subscriber.try_receive() {
            let schema_hash = sample.identity().map_or(0, |identity| identity.schema_hash);
            assert_ne!(schema_hash, 0, "a frame must carry a schema hash");
            return schema_hash;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no sample received");
}

#[test]
fn a_frame_hash_resolves_to_the_complete_published_identity() {
    let publisher_node = BusNode::new(&unique_name("ident_pub")).expect("publisher node");
    let reader_node = BusNode::new(&unique_name("ident_sub")).expect("reader node");
    let topic = format!("/{}", unique_name("ident_topic"));
    let data_type = format!("{}/Type", unique_name("ident_type"));
    let md5 = "d9d213e8c146065ec958333c617eb1a6";
    let definition = "uint32 value";

    let mut publisher =
        publisher_node.publish_raw(&topic, &data_type, md5, definition).expect("publisher");
    let mut subscriber = reader_node.subscribe_raw(&topic, 1).expect("subscriber");
    let schema_hash = first_schema_hash(&mut publisher, &mut subscriber);

    let cache = SchemaIdentityCache::new();
    let mut resolved = None;
    for _ in 0..50 {
        resolved =
            cache.resolve(&reader_node, &topic, schema_hash, Instant::now()).expect("lookup");
        if resolved.is_some() {
            break;
        }
        std::thread::sleep(RETRY_INTERVAL);
    }

    let identity = resolved.expect("retained identity");
    assert_eq!(identity.data_type, data_type);
    assert_eq!(identity.md5, md5);
    assert_eq!(identity.definition, definition);
}

#[test]
fn a_resolved_identity_is_served_from_the_cache() {
    let publisher_node = BusNode::new(&unique_name("cached_pub")).expect("publisher node");
    let reader_node = BusNode::new(&unique_name("cached_sub")).expect("reader node");
    let topic = format!("/{}", unique_name("cached_topic"));
    let data_type = format!("{}/Type", unique_name("cached_type"));
    let md5 = "d9d213e8c146065ec958333c617eb1a6";

    let mut publisher =
        publisher_node.publish_raw(&topic, &data_type, md5, "uint32 value").expect("publisher");
    let mut subscriber = reader_node.subscribe_raw(&topic, 1).expect("subscriber");
    let schema_hash = first_schema_hash(&mut publisher, &mut subscriber);

    let cache = SchemaIdentityCache::new();
    for _ in 0..50 {
        let found =
            cache.resolve(&reader_node, &topic, schema_hash, Instant::now()).expect("lookup");
        if found.is_some() {
            break;
        }
        std::thread::sleep(RETRY_INTERVAL);
    }

    assert!(cache.cached(&topic, schema_hash).is_some(), "a resolved identity must be retained");
    // The publisher is gone, so only the cache can still answer.
    drop(publisher);
    drop(publisher_node);
    let identity = cache.cached(&topic, schema_hash).expect("cached identity");
    assert_eq!(identity.data_type, data_type);
}

#[test]
fn an_unresolvable_hash_is_not_retried_on_every_sample() {
    let node = BusNode::new(&unique_name("retry_node")).expect("node");
    let topic = format!("/{}", unique_name("retry_topic"));
    let cache = SchemaIdentityCache::new();
    let now = Instant::now();

    let first = cache.resolve(&node, &topic, 0x1234_5678_9abc_def0, now).expect("first lookup");
    assert!(first.is_none(), "an unknown hash has no identity");

    let started = Instant::now();
    for _ in 0..20 {
        let repeat = cache.resolve(&node, &topic, 0x1234_5678_9abc_def0, now).expect("repeat");
        assert!(repeat.is_none());
    }
    assert!(
        started.elapsed() < RETRY_INTERVAL,
        "a paced retry must not query the transport for every sample"
    );
}

#[test]
fn a_zero_hash_never_reaches_the_transport() {
    let node = BusNode::new(&unique_name("zero_node")).expect("node");
    let cache = SchemaIdentityCache::new();

    let started = Instant::now();
    let resolved = cache.resolve(&node, "/anything", 0, Instant::now()).expect("zero hash");

    assert!(resolved.is_none());
    assert!(started.elapsed() < Duration::from_millis(20), "a zero hash must short-circuit");
}

#[test]
fn a_topic_whose_hash_never_resolves_does_not_stall_the_drain_loop() {
    // A live publisher serves its topic's schema, so a lookup waits for the resolver timeout
    // instead of failing fast.
    let publisher_node = BusNode::new(&unique_name("auto_schema_pub")).expect("publisher node");
    let topic = format!("/{}", unique_name("auto_schema_topic"));
    let mut publisher = publisher_node
        .publish_raw(&topic, "std_msgs/Float64", "fdb28210bfa9d7c91146260178d9a584", "float64 data")
        .expect("publisher");
    assert!(publisher.publish(&[1, 2, 3, 4]), "the publisher must reach the bus");

    let reader_node = BusNode::new(&unique_name("auto_schema_reader")).expect("reader node");
    let resolver = SchemaIdentityCache::new();

    // The first frame of an unresolvable hash may pay one lookup.
    let first = resolver
        .resolve(&reader_node, &topic, 0x0bad_0bad_0bad_0bad, Instant::now())
        .expect("first lookup");
    assert!(first.is_none(), "a hash no publisher retained has no identity");

    // Every later frame arriving inside the retry window must be cheap.
    let started = Instant::now();
    for _ in 0..5 {
        let repeat = resolver
            .resolve(&reader_node, &topic, 0x0bad_0bad_0bad_0bad, Instant::now())
            .expect("repeat lookup");
        assert!(repeat.is_none());
    }
    assert!(
        started.elapsed() < RETRY_INTERVAL,
        "5 further frames of one unresolvable topic cost {:?}; a paced resolver \
         must not stall the drain loop once per frame",
        started.elapsed()
    );
}
