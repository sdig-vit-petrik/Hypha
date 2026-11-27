//! The probe a node's housekeeping runs for the topics it subscribes to: the records of a
//! topic list its streams, and one open per stream reads its live publishers. Nothing is
//! kept open, so a version that left leaves no record, no descriptor and no policy.

#![allow(clippy::expect_used, clippy::print_stdout, clippy::panic)]

use std::time::Instant;

use hypha_core::port::{PortError, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::{Iox2Transport, topic_records};
use hypha_testkit::unique_name;

const V1_MD5: &str = "d9d213e8c146065ec958333c617eb1a6";

fn md5_of(k: u32) -> String {
    format!("{k:08x}{}", "0".repeat(24))
}

fn declared() -> QosRequest {
    QosRequest::Declared(hypha_iceoryx2::qos::default_policy())
}

fn v1(topic: &str) -> TopicSpec {
    TopicSpec::new(topic)
        .with_type("rv/Kind")
        .with_md5(V1_MD5)
        .with_definition("uint32 value")
        .keeping(4)
        .with_qos(declared())
}

/// Version `k`, an endpoint without a definition.
fn vk(topic: &str, k: u32) -> TopicSpec {
    TopicSpec::new(topic).with_type("rv/Kind").with_md5(&md5_of(k)).keeping(4).with_qos(declared())
}

/// What a refresh of `topic` finds: the schema hashes with a live publisher, other than
/// version 1's.
fn others(transport: &Iox2Transport, topic: &str) -> Vec<u64> {
    let own = hypha_wire::schema_hash_from_md5(V1_MD5).expect("hash");
    topic_records(topic)
        .expect("records")
        .into_iter()
        .filter(|stream| stream.schema_hash != own)
        .filter(|stream| {
            let live = transport.live_publishers(topic, stream.schema_hash).expect("probe");
            live.is_some_and(|live| live > 0)
        })
        .map(|stream| stream.schema_hash)
        .collect()
}

fn record_files(topic: &str) -> usize {
    let dir = hypha_iceoryx2::schema_file_path(topic, 0).expect("path");
    let dir = dir.parent().expect("topic directory");
    std::fs::read_dir(dir).map_or(0, Iterator::count)
}

fn open_fds() -> usize {
    std::fs::read_dir("/dev/fd").map_or(0, Iterator::count)
}

#[test]
fn a_subscribed_only_version_is_not_reported_as_published() {
    let transport = Iox2Transport::with_name(&unique_name("probe_subonly")).expect("transport");
    let topic = format!("/{}", unique_name("probe_subonly"));
    let _reader = transport.subscribe(&v1(&topic)).expect("v1 reader");
    let _v2_reader = transport.subscribe(&vk(&topic, 2)).expect("v2 reader");
    assert!(others(&transport, &topic).is_empty());
}

#[test]
fn a_version_without_a_definition_is_listed_by_its_hash() {
    let transport = Iox2Transport::with_name(&unique_name("probe_bare")).expect("transport");
    let topic = format!("/{}", unique_name("probe_bare"));
    let _reader = transport.subscribe(&v1(&topic)).expect("v1 reader");
    let _v2 = transport.publish(&vk(&topic, 2)).expect("v2 without a definition");
    let hash = hypha_wire::schema_hash_from_md5(&md5_of(2)).expect("hash");
    assert_eq!(others(&transport, &topic), [hash]);
}

/// An endpoint without a definition records its stream first; a later one with the
/// definition completes the record, so a lookup resolves it.
#[test]
fn a_record_without_a_definition_is_completed_by_one_with_it() {
    let transport = Iox2Transport::with_name(&unique_name("probe_upgrade")).expect("transport");
    let topic = format!("/{}", unique_name("probe_upgrade"));
    let bare = TopicSpec::new(&topic).with_type("rv/Kind").with_md5(V1_MD5);
    let _opener = transport.publish(&bare).expect("an opener without a definition");
    assert!(topic_records(&topic).expect("records").iter().all(|stream| stream.md5.is_empty()));
    let _full = transport.publish(&v1(&topic)).expect("a publisher with the definition");
    let named: Vec<String> =
        topic_records(&topic).expect("records").into_iter().map(|stream| stream.md5).collect();
    assert_eq!(named, [V1_MD5]);
}

#[test]
fn a_version_that_left_leaves_nothing_on_disk() {
    let transport = Iox2Transport::with_name(&unique_name("probe_leak")).expect("transport");
    let other = Iox2Transport::with_name(&unique_name("probe_leak_pub")).expect("transport");
    let topic = format!("/{}", unique_name("probe_leak"));
    let reader = transport.subscribe(&v1(&topic)).expect("v1 reader");
    let v2 = other.publish(&vk(&topic, 2)).expect("v2 publisher");
    assert_eq!(others(&transport, &topic).len(), 1);
    drop(v2);
    assert!(others(&transport, &topic).is_empty());
    assert_eq!(record_files(&topic), 1, "only version 1's record is left");
    drop(reader);
    assert_eq!(record_files(&topic), 0, "the files of a topic nobody uses go");
}

#[test]
fn versions_that_come_and_go_leave_no_descriptor_and_no_record() {
    let transport = Iox2Transport::with_name(&unique_name("probe_churn")).expect("transport");
    let other = Iox2Transport::with_name(&unique_name("probe_churn_pub")).expect("transport");
    let topic = format!("/{}", unique_name("probe_churn"));
    let _reader = transport.subscribe(&v1(&topic)).expect("v1 reader");
    let _ = others(&transport, &topic);
    let fds_before = open_fds();
    for k in 1..=100 {
        let publisher = other.publish(&vk(&topic, 100 + k)).expect("vk");
        assert_eq!(others(&transport, &topic).len(), 1);
        drop(publisher);
        assert!(others(&transport, &topic).is_empty());
    }
    let fds_after = open_fds();
    println!("descriptors {fds_before} -> {fds_after}; records {}", record_files(&topic));
    assert_eq!(record_files(&topic), 1);
    // A few for the publishing session's first ports, none for each of the 100 versions.
    assert!(fds_after < fds_before.saturating_add(10), "{fds_before} -> {fds_after}");
}

#[test]
fn a_version_that_left_takes_its_policy_with_it() {
    let transport = Iox2Transport::with_name(&unique_name("probe_policy")).expect("transport");
    let other = Iox2Transport::with_name(&unique_name("probe_policy_pub")).expect("transport");
    let topic = format!("/{}", unique_name("probe_policy"));
    let small = QosRequest::Declared(QosPolicy::new("small", 1, 2, 3));
    let _reader = transport.subscribe(&v1(&topic)).expect("v1 reader");
    let first = other.publish(&vk(&topic, 3).with_qos(small)).expect("v2 small");
    assert_eq!(others(&transport, &topic).len(), 1);
    drop(first);
    let again = other.publish(&vk(&topic, 3));
    assert!(again.is_ok(), "{:?}", again.err());
}

#[test]
#[ignore = "the peer of a_killed_publisher_of_another_version_is_not_reported"]
fn probe_peer_publishes_and_hangs() {
    let Ok(topic) = std::env::var("PROBE_TOPIC") else { return };
    let transport = Iox2Transport::with_name(&unique_name("probe_peer")).expect("transport");
    let _publisher = transport.publish(&vk(&topic, 7)).expect("vk");
    println!("ready");
    std::thread::sleep(std::time::Duration::from_secs(60));
}

#[test]
fn a_killed_publisher_of_another_version_is_not_reported() {
    use std::io::{BufRead, BufReader};
    let transport = Iox2Transport::with_name(&unique_name("probe_crash")).expect("transport");
    let topic = format!("/{}", unique_name("probe_crash"));
    let _reader = transport.subscribe(&v1(&topic)).expect("v1 reader");
    let mut child = std::process::Command::new(std::env::current_exe().expect("binary"))
        .args(["--ignored", "--exact", "probe_peer_publishes_and_hangs", "--nocapture"])
        .env("PROBE_TOPIC", &topic)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("peer");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    assert!(lines.any(|line| line.is_ok_and(|line| line.contains("ready"))));
    assert_eq!(others(&transport, &topic).len(), 1);
    child.kill().expect("kill");
    child.wait().expect("wait");
    let listed: Vec<usize> = transport
        .list_service_ports()
        .expect("listing")
        .into_iter()
        .filter(|service| service.name == topic)
        .map(|service| service.publishers)
        .collect();
    assert!(others(&transport, &topic).is_empty(), "a dead node's port is no publisher");
    assert!(listed.iter().all(|publishers| *publishers == 0), "{listed:?}");
}

/// What one refresh of a watched topic costs with `n` other streams, each with a live
/// publisher: one read of the records, and per stream one open and the node states of
/// that one service.
#[test]
fn the_cost_of_a_refresh_with_n_other_streams() {
    for n in [0_u32, 1, 4, 32] {
        let transport = Iox2Transport::with_name(&unique_name("probe_cost")).expect("transport");
        let other = Iox2Transport::with_name(&unique_name("probe_cost_pub")).expect("transport");
        let topic = format!("/{}", unique_name("probe_cost"));
        let _reader = transport.subscribe(&v1(&topic)).expect("v1 reader");
        let _publishers: Vec<_> =
            (0..n).map(|k| other.publish(&vk(&topic, 1000 + k)).expect("vk")).collect();
        let rounds = 20_u32;
        let start = Instant::now();
        for _ in 0..rounds {
            let _ = topic_records(&topic).expect("records");
        }
        let records = start.elapsed() / rounds;
        let start = Instant::now();
        for _ in 0..rounds {
            assert_eq!(others(&transport, &topic).len(), n as usize);
        }
        println!(
            "refresh with {n} other streams: {:?}, of which records {records:?}",
            start.elapsed() / rounds,
        );
    }
}

/// Version `k` with `policy`, a reader of one sample.
fn tight(topic: &str, k: u32, policy: &QosPolicy) -> TopicSpec {
    vk(topic, k).keeping(1).with_qos(QosRequest::Declared(policy.clone()))
}

/// A probe's open takes a node slot of the stream (`max_nodes` is `max_publishers` plus the
/// subscribers the stream admits): on a stream whose slots are all taken the probe is
/// refused, and a refresh keeps its last result for the topic.
#[test]
fn a_probe_of_a_stream_whose_node_slots_are_taken_is_refused() {
    let topic = format!("/{}", unique_name("probe_full"));
    let policy = QosPolicy::new("tight", 1, 1, 1);
    let publisher_node = Iox2Transport::with_name(&unique_name("probe_full_pub")).expect("node");
    let _publisher = publisher_node.publish(&tight(&topic, 2, &policy)).expect("v2 publisher");
    // Node slots: max_publishers + max_subscribers + 3 observer slots = 5.
    let mut nodes = Vec::new();
    let mut holders = Vec::new();
    let reader_node = Iox2Transport::with_name(&unique_name("probe_full_sub")).expect("node");
    holders.push(reader_node.subscribe(&tight(&topic, 2, &policy)).expect("declared reader"));
    nodes.push(reader_node);
    for _ in 0..3 {
        let node = Iox2Transport::with_name(&unique_name("probe_full_obs")).expect("node");
        let observer = TopicSpec::new(&topic).with_type("rv/Kind").with_md5(&md5_of(2)).keeping(1);
        holders.push(node.subscribe(&observer).expect("observer"));
        nodes.push(node);
    }
    let watcher = Iox2Transport::with_name(&unique_name("probe_full_watch")).expect("node");
    let hash = hypha_wire::schema_hash_from_md5(&md5_of(2)).expect("hash");
    let probed = watcher.live_publishers(&topic, hash);
    assert!(matches!(probed, Err(PortError::ResourceLimit)), "{probed:?}");
}

/// While a probe's open lasts it holds a node slot, so an endpoint that needs the last slot
/// of the stream at that instant is refused with `ResourceLimit`.
#[test]
fn a_probe_can_hold_the_last_node_slot_an_endpoint_needs() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let topic = format!("/{}", unique_name("probe_slot"));
    let policy = QosPolicy::new("tight", 1, 1, 1);
    let publisher_node = Iox2Transport::with_name(&unique_name("probe_slot_pub")).expect("node");
    let _publisher = publisher_node.publish(&tight(&topic, 2, &policy)).expect("v2 publisher");
    // Four of the five node slots taken: the reader below and a probe compete for the last.
    let mut nodes = Vec::new();
    let mut holders = Vec::new();
    for _ in 0..3 {
        let node = Iox2Transport::with_name(&unique_name("probe_slot_obs")).expect("node");
        let observer = TopicSpec::new(&topic).with_type("rv/Kind").with_md5(&md5_of(2)).keeping(1);
        holders.push(node.subscribe(&observer).expect("observer"));
        nodes.push(node);
    }
    let hash = hypha_wire::schema_hash_from_md5(&md5_of(2)).expect("hash");
    let stop = Arc::new(AtomicBool::new(false));
    let probing = {
        let stop = Arc::clone(&stop);
        let topic = topic.clone();
        std::thread::spawn(move || {
            let probes: Vec<_> = (0..2)
                .map(|_| Iox2Transport::with_name(&unique_name("probe_slot_probe")).expect("node"))
                .collect();
            while !stop.load(Ordering::Relaxed) {
                for probe in &probes {
                    let _ = probe.live_publishers(&topic, hash);
                }
            }
        })
    };
    let reader_node = Iox2Transport::with_name(&unique_name("probe_slot_sub")).expect("node");
    let mut refused = 0_u32;
    let deadline =
        Instant::now().checked_add(std::time::Duration::from_secs(10)).expect("deadline");
    while refused == 0 && Instant::now() < deadline {
        match reader_node.subscribe(&tight(&topic, 2, &policy)) {
            Ok(reader) => drop(reader),
            Err(PortError::ResourceLimit) => refused = refused.saturating_add(1),
            Err(other) => panic!("{other:?}"),
        }
    }
    stop.store(true, Ordering::Relaxed);
    probing.join().expect("prober");
    assert!(refused > 0, "no probe ever held the reader's slot in 10 s");
}

#[test]
#[ignore = "the peer of a_killed_publishers_record_goes_with_the_reclaim_of_its_node"]
fn probe_peer_publishes_under_a_name() {
    let (Ok(topic), Ok(name)) = (std::env::var("PROBE_TOPIC"), std::env::var("PROBE_NAME")) else {
        return;
    };
    let transport = Iox2Transport::with_name(&name).expect("transport");
    let _publisher = std::env::var("PROBE_PUBLISH")
        .is_ok()
        .then(|| transport.publish(&vk(&topic, 7)).expect("v7"));
    println!("ready");
    std::thread::sleep(std::time::Duration::from_secs(60));
}

/// A peer process named `name`, publishing version 7 on `topic` when `publish`.
fn named_peer(topic: &str, name: &str, publish: bool) -> std::process::Child {
    use std::io::{BufRead, BufReader};
    let mut command = std::process::Command::new(std::env::current_exe().expect("binary"));
    command
        .args(["--ignored", "--exact", "probe_peer_publishes_under_a_name", "--nocapture"])
        .env("PROBE_TOPIC", topic)
        .env("PROBE_NAME", name)
        .stdout(std::process::Stdio::piped());
    if publish {
        command.env("PROBE_PUBLISH", "1");
    }
    let mut child = command.spawn().expect("peer");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    assert!(lines.any(|line| line.is_ok_and(|line| line.contains("ready"))));
    child
}

/// A publisher killed with SIGKILL leaves its stream's record. Once the node start of its
/// name reclaimed its ports, and the service went with them, the next probe of the stream,
/// as a refresh makes, removes the record.
#[test]
fn a_killed_publishers_record_goes_once_its_stream_is_reclaimed() {
    let topic = format!("/{}", unique_name("probe_reclaim"));
    let name = unique_name("probe_reclaim_peer");
    let hash = hypha_wire::schema_hash_from_md5(&md5_of(7)).expect("hash");
    let mut child = named_peer(&topic, &name, true);
    child.kill().expect("kill");
    child.wait().expect("wait");
    let recorded = |topic: &str| -> Vec<u64> {
        topic_records(topic).expect("records").iter().map(|stream| stream.schema_hash).collect()
    };
    assert_eq!(recorded(&topic), [hash], "the killed endpoint could not remove its record");
    // The node starts again under its name, without the publisher: it reclaims the dead run.
    let mut again = named_peer(&topic, &name, false);
    let watcher = Iox2Transport::with_name(&unique_name("probe_reclaim_watch")).expect("node");
    assert_eq!(watcher.live_publishers(&topic, hash).expect("probe"), None);
    let after = recorded(&topic);
    again.kill().expect("kill");
    again.wait().expect("wait");
    assert!(after.is_empty(), "the reclaimed stream's record is gone: {after:x?}");
}
