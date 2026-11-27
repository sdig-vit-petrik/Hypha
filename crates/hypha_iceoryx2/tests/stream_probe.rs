//! The probe a node's housekeeping runs for the topics it subscribes to: the records of a
//! topic list its streams, and one open per stream reads its live publishers. Nothing is
//! kept open, so a version that left leaves no record, no descriptor and no policy.

#![allow(clippy::expect_used, clippy::print_stdout)]

use std::time::Instant;

use hypha_core::port::{QosPolicy, QosRequest, TopicSpec};
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
    let dead = transport.dead_nodes().expect("dead nodes");
    topic_records(topic)
        .expect("records")
        .into_iter()
        .filter(|stream| stream.schema_hash != own)
        .filter(|stream| {
            let ports = transport.publisher_ports(topic, stream.schema_hash).expect("probe");
            ports.is_some_and(|ports| ports.live(&dead) > 0)
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
/// publisher: one read of the records and of the dead nodes, and one open per stream.
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
            let _ = transport.dead_nodes().expect("dead nodes");
        }
        let dead = start.elapsed() / rounds;
        let start = Instant::now();
        for _ in 0..rounds {
            for stream in topic_records(&topic).expect("records") {
                let _ = transport.publisher_ports(&topic, stream.schema_hash).expect("probe");
            }
        }
        let opens = start.elapsed() / rounds;
        let start = Instant::now();
        for _ in 0..rounds {
            assert_eq!(others(&transport, &topic).len(), n as usize);
        }
        println!(
            "refresh with {n} other streams: {:?}: records {records:?}, records and one open per \
             stream {opens:?}, dead nodes {dead:?}",
            start.elapsed() / rounds,
        );
    }
}
