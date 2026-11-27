//! Topic name overloading (#209): one topic name carries more than one schema, and each
//! schema is its own stream: a typed subscriber never meets a frame of another schema. The
//! case that matters is a migration, two versions of one message type (one type name, two
//! MD5s) on one topic. These tests observe it through the Rust SDK, with two types and with
//! two versions of one type, in one process and in two.
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

/// Version 1 of a message under migration: the same type name as [`VERSION_2`], one field.
const VERSION_1: Kind = Kind {
    data_type: "overload_test/Reading",
    md5: "b3087778e93fcd34cc8d65bc54e850d1",
    definition: "int32 value\n",
};

/// Version 2: two fields more, so another MD5 and a longer payload.
const VERSION_2: Kind = Kind {
    data_type: "overload_test/Reading",
    md5: "f79835def461350458ead76f07016dd7",
    definition: "int32 value\nstring label\nfloat64 weight\n",
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
fn the_frames_of_the_other_type_never_reach_a_typed_subscriber_and_nothing_is_counted() {
    let bus = BusNode::new(&unique_name("overload_refusals")).expect("node");
    let topic = format!("/overload/{}", unique_name("refusals"));
    let mut first_out = publisher(&bus, &topic, &FIRST);
    let mut second_out = publisher(&bus, &topic, &SECOND);
    let mut first_in = typed_subscriber(&bus, &topic, &FIRST);

    for index in 0_u8..4 {
        assert!(second_out.publish(&[index, 0, 0, 0, 0, 0, 0, 0]));
    }
    for index in 0_u8..3 {
        assert!(first_out.publish(&[index, 0, 0, 0]));
    }
    let seen = drain_until(&mut first_in, 3);
    std::thread::sleep(Duration::from_millis(20));
    let after = drain(&mut first_in);

    // The other type is another stream: its frames are no refusal, no schema mismatch.
    assert_eq!(seen.payloads.len(), 3);
    assert_eq!((seen.refusals, after.refusals, after.payloads.len()), (0, 0, 0));
    for counter in [
        "integrity.schema_mismatch",
        "integrity.sequence_gap",
        "integrity.invalid_frame",
        "integrity.corrupt_payload",
    ] {
        assert_eq!(metric(&first_in, counter), 0, "{counter}");
    }
    assert_eq!(first_in.missing_sequences().expect("count"), 0);
}

/// A frame whose header names another schema than the stream it travels on is a fault, and
/// still counted as one: here a transport publisher of version 1's stream stamps version 2.
#[test]
fn a_frame_of_another_schema_than_its_stream_is_still_a_schema_mismatch() {
    use hypha_core::port::TopicSpec;
    let bus = BusNode::new(&unique_name("overload_wrong_hash")).expect("node");
    let topic = format!("/overload/{}", unique_name("wrong_hash"));
    let transport = hypha_iceoryx2::Iox2Transport::with_name(&unique_name("overload_wrong_sender"))
        .expect("transport");
    let spec = TopicSpec::new(&topic)
        .with_type(VERSION_1.data_type)
        .with_md5(VERSION_1.md5)
        .with_definition(VERSION_1.definition);
    let wrong = transport.publish(&spec).expect("publisher of version 1's stream");
    let mut v1_in = typed_subscriber(&bus, &topic, &VERSION_1);

    let now = u64::try_from(hypha_time::steady_now().nanos()).expect("a steady time");
    let mut loan = wrong.loan(4).expect("loan");
    loan.as_mut_bytes().copy_from_slice(&payload_of(&VERSION_1, 1));
    let metadata = hypha_wire::frame::FrameMetadata {
        schema_hash: schema_hash(&VERSION_2),
        publisher_id: 1,
        sequence: 1,
        transport_sent_nanos: now,
        framework_sent_nanos: now,
    };
    loan.send_frame(metadata, 0).expect("send");

    let deadline = deadline_in(20);
    while metric(&v1_in, "integrity.schema_mismatch") == 0 {
        assert!(Instant::now() < deadline, "the wrong frame was not refused");
        assert!(drain(&mut v1_in).payloads.is_empty());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(metric(&v1_in, "integrity.schema_mismatch"), 1);
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
fn the_bus_lists_one_stream_for_each_version_of_a_name() {
    let bus = BusNode::new(&unique_name("overload_list")).expect("node");
    let reader = BusNode::new(&unique_name("overload_list_reader")).expect("node");
    let topic = format!("/overload/{}", unique_name("list"));
    let _first_out = publisher(&bus, &topic, &VERSION_1);
    let _second_out = publisher(&bus, &topic, &VERSION_2);

    let listed = reader.list_topics().expect("topics");
    let mut hashes: Vec<u64> = listed
        .iter()
        .filter(|entry| entry.name == topic && entry.published)
        .map(|entry| entry.schema_hash)
        .collect();
    hashes.sort_unstable();
    let mut expected = [schema_hash(&VERSION_1), schema_hash(&VERSION_2)];
    expected.sort_unstable();
    assert_eq!(hashes, expected);
}

fn schema_hash(kind: &Kind) -> u64 {
    hypha_wire::schema_hash_from_md5(kind.md5).expect("a schema hash")
}

/// The peer of the two-process tests: it publishes one type on the topic named by
/// `HYPHA_OVERLOAD_TOPIC`, by `HYPHA_OVERLOAD_KIND` (`first`, `version_1`, `version_2`, else
/// the second type), as many frames as `HYPHA_OVERLOAD_FRAMES` says (40),
/// `HYPHA_OVERLOAD_PERIOD_MS` apart (25). It does nothing when it runs on its own; the tests
/// below start it as a process of its own, with `--ignored`, and so can a person who wants
/// traffic of two types for a tool.
#[test]
#[ignore = "the peer process of two_processes_publishing_two_types_on_one_name_are_read_by_type"]
fn overload_peer_publishes_one_type() {
    let Ok(topic) = std::env::var("HYPHA_OVERLOAD_TOPIC") else { return };
    let setting = |name: &str, default: u64| {
        std::env::var(name).ok().and_then(|text| text.parse().ok()).unwrap_or(default)
    };
    let kind = match std::env::var("HYPHA_OVERLOAD_KIND").as_deref() {
        Ok("first") => &FIRST,
        Ok("version_1") => &VERSION_1,
        Ok("version_2") => &VERSION_2,
        _ => &SECOND,
    };
    let bus = BusNode::new(&unique_name("overload_peer")).expect("peer node");
    let mut out = publisher(&bus, &topic, kind);
    for index in 0..setting("HYPHA_OVERLOAD_FRAMES", 40) {
        let tag = u8::try_from(index % 100).unwrap_or(0);
        let payload = payload_of(kind, tag);
        assert!(out.publish(&payload));
        std::thread::sleep(Duration::from_millis(setting("HYPHA_OVERLOAD_PERIOD_MS", 25)));
    }
}

/// A payload of `kind` tagged `tag`, of a length of its own: four bytes for the first
/// type and version 1, sixteen for version 2, eight for the second type.
fn payload_of(kind: &Kind, tag: u8) -> Vec<u8> {
    let length = match kind.md5 {
        md5 if md5 == FIRST.md5 || md5 == VERSION_1.md5 => 4,
        md5 if md5 == VERSION_2.md5 => 16,
        _ => 8,
    };
    let mut payload = vec![0; length];
    if let Some(first) = payload.first_mut() {
        *first = tag;
    }
    payload
}

/// A typed subscriber of `kind` on `topic`, once a publisher opened the stream of `kind`:
/// a subscriber that declares no policy does not create a stream.
fn subscribe_once_published(bus: &BusNode, topic: &str, kind: &Kind) -> RawSubscriber {
    let deadline = deadline_in(20);
    loop {
        let options = SubscriberOptions::default().keeping(16);
        match bus.subscribe_raw_with_schema(topic, kind.data_type, kind.md5, options) {
            Ok(subscriber) => return subscriber,
            Err(error) => {
                assert!(Instant::now() < deadline, "{} never published: {error}", kind.md5);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Starts the peer process publishing `kind` on `topic`, `frames` frames.
fn start_peer(topic: &str, kind: &str, frames: u32) -> std::process::Child {
    let exe = std::env::current_exe().expect("the test binary");
    Command::new(exe)
        .args(["--ignored", "--exact", "overload_peer_publishes_one_type"])
        .env("HYPHA_OVERLOAD_TOPIC", topic)
        .env("HYPHA_OVERLOAD_KIND", kind)
        .env("HYPHA_OVERLOAD_FRAMES", frames.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the peer process")
}

#[test]
fn two_processes_publishing_two_types_on_one_name_are_read_by_type() {
    let bus = BusNode::new(&unique_name("overload_two_processes")).expect("node");
    let topic = format!("/overload/{}", unique_name("two_processes"));
    let mut first_out = publisher(&bus, &topic, &FIRST);
    let mut first_in = typed_subscriber(&bus, &topic, &FIRST);
    let mut peer = start_peer(&topic, "second", 400);
    let mut second_in = subscribe_once_published(&bus, &topic, &SECOND);

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
    assert_eq!((first_seen.refusals, second_seen.refusals), (0, 0), "no frame of the other type");
}

/// A migration, each publisher in a process of its own: version 1 runs, version 2 joins
/// on the same topic, each reader gets its own version alone and nothing is counted as a
/// fault; then version 1's publisher goes away and its reader, which keeps running, has
/// no publisher while version 2 still flows.
#[test]
fn two_versions_of_one_type_share_a_topic_through_a_migration() {
    let bus = BusNode::new(&unique_name("overload_migration")).expect("node");
    let topic = format!("/overload/{}", unique_name("migration"));
    let mut v1_peer = start_peer(&topic, "version_1", 400);
    let mut v1_in = subscribe_once_published(&bus, &topic, &VERSION_1);
    let v1_seen = drain_until(&mut v1_in, 3);

    let mut v2_peer = start_peer(&topic, "version_2", 400);
    let mut v2_in = subscribe_once_published(&bus, &topic, &VERSION_2);
    let v2_seen = drain_until(&mut v2_in, 3);
    let v1_beside = drain_until(&mut v1_in, 3);
    for (seen, length) in [(&v1_seen, 4), (&v1_beside, 4), (&v2_seen, 16)] {
        assert!(seen.payloads.iter().all(|payload| payload.len() == length), "{seen:?}");
    }

    let _ = v1_peer.kill();
    let _ = v1_peer.wait();
    let deadline = deadline_in(20);
    while v1_in.publisher_count().expect("count") > 0 {
        assert!(Instant::now() < deadline, "version 1's publisher never went away");
        std::thread::sleep(Duration::from_millis(10));
    }
    drain(&mut v1_in);
    let v2_later = drain_until(&mut v2_in, 3);
    std::thread::sleep(Duration::from_millis(100));
    let v1_later = drain(&mut v1_in);
    let _ = v2_peer.kill();
    let _ = v2_peer.wait();

    assert!(v1_later.payloads.is_empty(), "{v1_later:?}");
    assert!(v2_later.payloads.iter().all(|payload| payload.len() == 16), "{v2_later:?}");
    for (subscriber, seen) in [(&v1_in, &v1_beside), (&v2_in, &v2_later)] {
        assert_eq!(seen.refusals, 0);
        assert_eq!(metric(subscriber, "integrity.schema_mismatch"), 0);
    }
    assert_eq!(v1_later.refusals, 0);
}
