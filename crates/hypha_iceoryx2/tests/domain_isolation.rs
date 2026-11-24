//! A domain separates whole systems on one host.

mod support;
use support::sealed_send;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

use hypha_core::port::{PortError, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::domain::is_domain_name;
use hypha_iceoryx2::{Iox2Transport, list_nodes};
use hypha_testkit::unique_name;

const ROLE: &str = "HYPHA_DOMAIN_TEST_ROLE";
const TOPIC: &str = "HYPHA_DOMAIN_TEST_TOPIC";
const NODE: &str = "HYPHA_DOMAIN_TEST_NODE";
const PUBLISH_FOR: Duration = Duration::from_secs(4);
const WAIT_FOR_SAMPLE: Duration = Duration::from_secs(3);
const LISTEN_FOR_LEAK: Duration = Duration::from_millis(1500);

fn spawn_child(role: &str, domain: Option<&str>, topic: &str, node: &str) -> std::process::Child {
    spawn_child_in(role, domain.map(OsStr::new), topic, node)
}

#[allow(clippy::expect_used)]
fn spawn_child_in(
    role: &str,
    domain: Option<&OsStr>,
    topic: &str,
    node: &str,
) -> std::process::Child {
    let mut command = Command::new(std::env::current_exe().expect("test binary"));
    command
        .args(["domain_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .env(TOPIC, topic)
        .env(NODE, node);
    match domain {
        Some(name) => command.env("HYPHA_DOMAIN", name),
        None => command.env_remove("HYPHA_DOMAIN"),
    };
    command.spawn().expect("spawn child")
}

#[allow(clippy::expect_used)]
fn run_child(role: &str, domain: Option<&str>, topic: &str, node: &str) -> ExitStatus {
    spawn_child(role, domain, topic, node).wait().expect("child exit")
}

#[allow(clippy::expect_used)]
fn publish_until_deadline(topic: &str, node: &str) {
    let transport = Iox2Transport::with_name(node).expect("transport");
    let publisher =
        transport.publish(&TopicSpec::new(topic).with_type("hypha_test/Data")).expect("publisher");
    let started = Instant::now();
    while started.elapsed() < PUBLISH_FOR {
        let mut loan = publisher.loan(4).expect("loan");
        loan.as_mut_bytes().copy_from_slice(b"ping");
        loan.send_frame(sealed_send::metadata(), 0).expect("send");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[allow(clippy::expect_used)]
fn received_within(topic: &str, node: &str, window: Duration) -> bool {
    let transport = Iox2Transport::with_name(node).expect("transport");
    let subscriber = transport
        .subscribe(
            &TopicSpec::new(topic)
                .with_type("hypha_test/Data")
                .with_qos(QosRequest::Declared(QosPolicy::new("default", 8, 16, 32))),
        )
        .expect("subscriber");
    let started = Instant::now();
    while started.elapsed() < window {
        if subscriber.try_receive().expect("receive").is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// The body of every child process. Without a role it does nothing.
#[test]
#[allow(clippy::expect_used)]
fn domain_child() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let topic = std::env::var(TOPIC).expect("topic");
    let node = std::env::var(NODE).expect("node");
    let passed = match role.as_str() {
        "publish" => {
            publish_until_deadline(&topic, &node);
            true
        }
        "expect_samples" => received_within(&topic, &unique_name("domain_sub"), WAIT_FOR_SAMPLE),
        "expect_no_samples" => {
            !received_within(&topic, &unique_name("domain_sub"), LISTEN_FOR_LEAK)
        }
        "expect_node_absent" => {
            let _transport =
                Iox2Transport::with_name(&unique_name("domain_list")).expect("transport");
            list_nodes().expect("node list").iter().all(|listed| listed.name != node)
        }
        "expect_node_present" => {
            let _transport =
                Iox2Transport::with_name(&unique_name("domain_list")).expect("transport");
            list_nodes().expect("node list").iter().any(|listed| listed.name == node)
        }
        "expect_refused" => matches!(Iox2Transport::new(), Err(PortError::InvalidDomain)),
        "expect_domain" => hypha_iceoryx2::domain::domain().expect("domain") == topic,
        _ => false,
    };
    assert!(passed, "the child role '{role}' failed");
}

/// A domain of this test run under the test root, which `clean-shm.sh --domain t_` clears.
fn test_domain(side: &str) -> String {
    format!("t_{}_{side}", std::process::id())
}

#[test]
#[allow(clippy::expect_used)]
fn two_domains_do_not_see_each_others_samples_or_nodes() {
    let topic = format!("/{}", unique_name("domain_topic"));
    let publisher_node = unique_name("domain_pub");
    let (left, right) = (test_domain("left"), test_domain("right"));
    let mut publisher = spawn_child("publish", Some(&left), &topic, &publisher_node);
    std::thread::sleep(Duration::from_millis(300));

    let same_domain = run_child("expect_samples", Some(&left), &topic, &publisher_node);
    let other_domain = run_child("expect_no_samples", Some(&right), &topic, &publisher_node);
    let listed_in_same = run_child("expect_node_present", Some(&left), &topic, &publisher_node);
    let absent_in_other = run_child("expect_node_absent", Some(&right), &topic, &publisher_node);

    assert!(publisher.wait().expect("publisher exit").success());
    assert!(same_domain.success(), "a subscriber in the same domain received nothing");
    assert!(other_domain.success(), "a subscriber in another domain received a sample");
    assert!(listed_in_same.success(), "the node list of the same domain misses the node");
    assert!(absent_in_other.success(), "the node list of another domain shows the node");
}

#[test]
fn an_invalid_domain_is_refused() {
    for invalid in ["Left", "1left", "left-side", "_left", "a234567890123456789012345678901234"] {
        let refused = run_child("expect_refused", Some(invalid), "", "unused");
        assert!(refused.success(), "HYPHA_DOMAIN='{invalid}' was not refused");
    }
}

#[test]
#[allow(clippy::expect_used)]
fn a_domain_that_is_not_utf8_is_refused() {
    let not_utf8 = OsStr::from_bytes(b"left\xff");
    let refused =
        spawn_child_in("expect_refused", Some(not_utf8), "", "unused").wait().expect("child exit");
    assert!(refused.success(), "a HYPHA_DOMAIN that is not UTF-8 was not refused");
}

#[test]
fn the_active_domain_is_reported() {
    let left = test_domain("left");
    assert!(run_child("expect_domain", Some(&left), &left, "unused").success());
    assert!(run_child("expect_domain", None, "", "unused").success());
}

#[test]
fn domain_names_follow_the_identifier_rule() {
    assert!(is_domain_name("left"));
    assert!(is_domain_name("cell_2"));
    assert!(is_domain_name("a2345678901234567890123456789012"));
    assert!(!is_domain_name(""));
    assert!(!is_domain_name("Left"));
    assert!(!is_domain_name("2left"));
    assert!(!is_domain_name("left side"));
    assert!(!is_domain_name("a23456789012345678901234567890123"));
}
