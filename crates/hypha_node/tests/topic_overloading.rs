//! Topic name overloading (#209): one topic name carries more than one message type, and
//! each type is its own stream. The transport keys a topic by its name alone; each frame
//! carries the hash of its schema, and a subscriber that names a type keeps only the frames
//! of that type. These tests observe it through the Rust SDK, with two types of two MD5s,
//! in one process and in two.
#![allow(clippy::expect_used, clippy::panic)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hypha_node::sdk::{
    BusNode, RawPublisher, RawSubscriber, SchemaIdentityCache, SubscriberOptions,
};
use hypha_testkit::unique_name;

/// Two types of one name: their identity, as a publisher declares it and a reader names it.
struct Kind {
    data_type: &'static str,
    md5: &'static str,
    definition: &'static str,
}

const FIRST: Kind = Kind {
    data_type: "overload_test/First",
    md5: "d9d213e8c146065ec958333c617eb1a6",
    definition: "uint32 value",
};

const SECOND: Kind = Kind {
    data_type: "overload_test/Second",
    md5: "581e4b1bda6a172aa19e1038a38e01eb",
    definition: "int32 value\nstring label\n",
};

/// The instant `seconds` from now: the bound of a wait.
fn deadline_in(seconds: u64) -> Instant {
    Instant::now().checked_add(Duration::from_secs(seconds)).expect("a deadline")
}

fn publisher(bus: &BusNode, topic: &str, kind: &Kind) -> RawPublisher {
    bus.publish_raw(topic, kind.data_type, kind.md5, kind.definition).expect("publisher")
}

fn typed_subscriber(bus: &BusNode, topic: &str, kind: &Kind) -> RawSubscriber {
    bus.subscribe_raw_with_schema(
        topic,
        kind.data_type,
        kind.md5,
        SubscriberOptions::default().keeping(16),
    )
    .expect("typed subscriber")
}

/// What one drain of a subscriber found: the payloads it returned and the refusals it met.
#[derive(Default, Debug)]
struct Drained {
    payloads: Vec<Vec<u8>>,
    refusals: usize,
}

fn drain(subscriber: &mut RawSubscriber) -> Drained {
    let mut drained = Drained::default();
    loop {
        match subscriber.try_receive() {
            Ok(Some(sample)) => {
                drained.payloads.push(sample.bytes().expect("payload").to_vec());
            }
            Ok(None) => return drained,
            Err(_) => drained.refusals = drained.refusals.saturating_add(1),
        }
    }
}

/// Drains `subscriber` until it holds `wanted` payloads, or fails after a bound.
fn drain_until(subscriber: &mut RawSubscriber, wanted: usize) -> Drained {
    let deadline = deadline_in(20);
    let mut total = Drained::default();
    while total.payloads.len() < wanted {
        assert!(
            Instant::now() < deadline,
            "only {} of {wanted} frames arrived",
            total.payloads.len()
        );
        let more = drain(subscriber);
        total.payloads.extend(more.payloads);
        total.refusals = total.refusals.saturating_add(more.refusals);
        std::thread::sleep(Duration::from_millis(5));
    }
    total
}

fn metric(subscriber: &RawSubscriber, name: &str) -> u64 {
    let list = subscriber.metrics().expect("metrics");
    list.iter().find(|(metric, _)| metric == name).map(|(_, value)| *value).expect("a metric")
}

#[test]
fn two_publishers_of_two_types_on_one_name_are_both_created() {
    let bus = BusNode::new(&unique_name("overload_create")).expect("node");
    let topic = format!("/overload/{}", unique_name("create"));

    let first = bus.publish_raw(&topic, FIRST.data_type, FIRST.md5, FIRST.definition);
    let second = bus.publish_raw(&topic, SECOND.data_type, SECOND.md5, SECOND.definition);

    assert!(first.is_ok(), "{:?}", first.err());
    assert!(second.is_ok(), "{:?}", second.err());
}

#[test]
fn each_typed_subscriber_of_a_name_returns_only_the_frames_of_its_type() {
    let bus = BusNode::new(&unique_name("overload_one_process")).expect("node");
    let topic = format!("/overload/{}", unique_name("one_process"));
    let mut first_out = publisher(&bus, &topic, &FIRST);
    let mut second_out = publisher(&bus, &topic, &SECOND);
    let mut first_in = typed_subscriber(&bus, &topic, &FIRST);
    let mut second_in = typed_subscriber(&bus, &topic, &SECOND);

    for index in 0_u8..5 {
        assert!(first_out.publish(&[index, 0, 0, 0]));
        assert!(second_out.publish(&[index.saturating_add(100), 0, 0, 0, 0, 0, 0, 0]));
    }

    let first_seen = drain_until(&mut first_in, 5);
    let second_seen = drain_until(&mut second_in, 5);
    let expected_first: Vec<Vec<u8>> = (0_u8..5).map(|index| vec![index, 0, 0, 0]).collect();
    let expected_second: Vec<Vec<u8>> =
        (0_u8..5).map(|index| vec![index.saturating_add(100), 0, 0, 0, 0, 0, 0, 0]).collect();
    assert_eq!(first_seen.payloads, expected_first);
    assert_eq!(second_seen.payloads, expected_second);
}

#[test]
fn the_frames_of_the_other_type_come_back_as_refusals_of_a_typed_receive_and_are_counted() {
    let bus = BusNode::new(&unique_name("overload_refusals")).expect("node");
    let topic = format!("/overload/{}", unique_name("refusals"));
    let mut first_out = publisher(&bus, &topic, &FIRST);
    let mut second_out = publisher(&bus, &topic, &SECOND);
    let mut first_in = typed_subscriber(&bus, &topic, &FIRST);

    for index in 0_u8..3 {
        assert!(first_out.publish(&[index, 0, 0, 0]));
    }
    for index in 0_u8..4 {
        assert!(second_out.publish(&[index, 0, 0, 0, 0, 0, 0, 0]));
    }
    let mut seen = Drained::default();
    let deadline = deadline_in(20);
    while seen.payloads.len() < 3 || seen.refusals < 4 {
        assert!(Instant::now() < deadline, "{seen:?}");
        let more = drain(&mut first_in);
        seen.payloads.extend(more.payloads);
        seen.refusals = seen.refusals.saturating_add(more.refusals);
        std::thread::sleep(Duration::from_millis(5));
    }

    // A raw receive reports each frame of the other type as an error to step past, and
    // counts it as a schema mismatch: no sequence gap, no lost frame.
    assert_eq!(seen.payloads.len(), 3);
    assert_eq!(seen.refusals, 4);
    assert_eq!(metric(&first_in, "integrity.schema_mismatch"), 4);
    assert_eq!(metric(&first_in, "integrity.sequence_gap"), 0);
    assert_eq!(first_in.missing_sequences().expect("count"), 0);
}

#[test]
fn an_untyped_subscriber_reads_both_types_and_each_hash_resolves_to_its_own_identity() {
    let bus = BusNode::new(&unique_name("overload_untyped")).expect("node");
    let reader = BusNode::new(&unique_name("overload_untyped_reader")).expect("node");
    let topic = format!("/overload/{}", unique_name("untyped"));
    let mut first_out = publisher(&bus, &topic, &FIRST);
    let mut second_out = publisher(&bus, &topic, &SECOND);
    let mut any_in = reader.subscribe_raw(&topic, 16).expect("untyped subscriber");

    assert!(first_out.publish(&[1, 0, 0, 0]));
    assert!(second_out.publish(&[2, 0, 0, 0, 0, 0, 0, 0]));

    let mut hashes = Vec::new();
    let deadline = deadline_in(20);
    while hashes.len() < 2 {
        assert!(Instant::now() < deadline, "only {} of 2 frames arrived", hashes.len());
        while let Ok(Some(sample)) = any_in.try_receive() {
            let identity = sample.identity().expect("identity");
            hashes.push(identity.schema_hash);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(hashes.len(), 2);
    assert_ne!(hashes.first(), hashes.last(), "two types are two schema hashes");

    let cache = SchemaIdentityCache::new();
    let mut types = Vec::new();
    for hash in hashes {
        let deadline = deadline_in(20);
        let resolved = cache
            .resolve_until(&reader, &topic, hash, deadline)
            .expect("lookup")
            .expect("retained identity");
        types.push((resolved.data_type.clone(), resolved.md5.clone()));
    }
    types.sort();
    assert_eq!(
        types,
        vec![
            (FIRST.data_type.to_owned(), FIRST.md5.to_owned()),
            (SECOND.data_type.to_owned(), SECOND.md5.to_owned()),
        ]
    );
}

#[test]
fn the_bus_lists_one_topic_for_a_name_of_two_types() {
    let bus = BusNode::new(&unique_name("overload_list")).expect("node");
    let reader = BusNode::new(&unique_name("overload_list_reader")).expect("node");
    let topic = format!("/overload/{}", unique_name("list"));
    let _first_out = publisher(&bus, &topic, &FIRST);
    let _second_out = publisher(&bus, &topic, &SECOND);

    let listed = reader.list_topics().expect("topics");
    let entries: Vec<_> = listed.iter().filter(|entry| entry.name == topic).collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(entries.iter().all(|entry| entry.published));
}

/// The peer of the two-process test: it publishes one type on the topic named by
/// `HYPHA_OVERLOAD_TOPIC`, the second unless `HYPHA_OVERLOAD_KIND` is `first`, as many
/// frames as `HYPHA_OVERLOAD_FRAMES` says (40), `HYPHA_OVERLOAD_PERIOD_MS` apart (25). It
/// does nothing when it runs on its own; the test below starts it as a process of its own,
/// with `--ignored`, and so can a person who wants traffic of two types for a tool.
#[test]
#[ignore = "the peer process of two_processes_publishing_two_types_on_one_name_are_read_by_type"]
fn overload_peer_publishes_one_type() {
    let Ok(topic) = std::env::var("HYPHA_OVERLOAD_TOPIC") else { return };
    let setting = |name: &str, default: u64| {
        std::env::var(name).ok().and_then(|text| text.parse().ok()).unwrap_or(default)
    };
    let first = std::env::var("HYPHA_OVERLOAD_KIND").is_ok_and(|kind| kind == "first");
    let kind = if first { &FIRST } else { &SECOND };
    let bus = BusNode::new(&unique_name("overload_peer")).expect("peer node");
    let mut out = publisher(&bus, &topic, kind);
    for index in 0..setting("HYPHA_OVERLOAD_FRAMES", 40) {
        let tag = u8::try_from(index % 100).unwrap_or(0);
        let payload: &[u8] =
            if first { &[tag, 0, 0, 0] } else { &[tag.saturating_add(100), 0, 0, 0, 0, 0, 0, 0] };
        assert!(out.publish(payload));
        std::thread::sleep(Duration::from_millis(setting("HYPHA_OVERLOAD_PERIOD_MS", 25)));
    }
}

#[test]
fn two_processes_publishing_two_types_on_one_name_are_read_by_type() {
    let bus = BusNode::new(&unique_name("overload_two_processes")).expect("node");
    let topic = format!("/overload/{}", unique_name("two_processes"));
    let mut first_out = publisher(&bus, &topic, &FIRST);
    let mut first_in = typed_subscriber(&bus, &topic, &FIRST);
    let mut second_in = typed_subscriber(&bus, &topic, &SECOND);

    let exe = std::env::current_exe().expect("the test binary");
    let mut peer = Command::new(exe)
        .args(["--ignored", "--exact", "overload_peer_publishes_one_type"])
        .env("HYPHA_OVERLOAD_TOPIC", &topic)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the peer process");

    let mut sent = 0_u8;
    let deadline = deadline_in(30);
    let mut first_seen = Drained::default();
    let mut second_seen = Drained::default();
    while first_seen.payloads.len() < 20 || second_seen.payloads.len() < 20 {
        if Instant::now() >= deadline {
            let _ = peer.kill();
            panic!("first {first_seen:?}, second {second_seen:?}");
        }
        if sent < 40 {
            assert!(first_out.publish(&[sent, 0, 0, 0]));
            sent = sent.saturating_add(1);
        }
        for (subscriber, seen) in
            [(&mut first_in, &mut first_seen), (&mut second_in, &mut second_seen)]
        {
            let more = drain(subscriber);
            seen.payloads.extend(more.payloads);
            seen.refusals = seen.refusals.saturating_add(more.refusals);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = peer.kill();
    let _ = peer.wait();

    assert!(first_seen.payloads.iter().all(|payload| payload.len() == 4), "{first_seen:?}");
    assert!(second_seen.payloads.iter().all(|payload| payload.len() == 8), "{second_seen:?}");
    // Each reader met the other process's or the other type's frames as refusals only.
    assert!(first_seen.refusals > 0, "the first type's reader met no frame of the second type");
    assert!(second_seen.refusals > 0, "the second type's reader met no frame of the first type");
}
