//! `hypha topic list` lists the streams of the bus from the transport's service listing (#178):
//! a topic that is advertised and has not published yet is listed with its type, as are the
//! topics that publish, and the command does not listen.
#![allow(clippy::expect_used, clippy::panic)]

use hypha_node::sdk::md5::md5_from_definition;
use hypha_node::sdk::{BusNode, RawPublisher};
use hypha_testkit::unique_name;
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DATA_TYPE: &str = "list_test/Reading";
const VERSION_1: &str = "int32 value\n";
const VERSION_2: &str = "int32 value\nstring label\n";

fn md5_of(definition: &str) -> String {
    md5_from_definition(DATA_TYPE, definition).expect("MD5")
}

fn advertise(bus: &BusNode, topic: &str, definition: &str) -> RawPublisher {
    bus.publish_raw(topic, DATA_TYPE, &md5_of(definition), definition).expect("a publisher")
}

fn setup(label: &str) -> (BusNode, String) {
    let name = unique_name(label);
    (BusNode::new(&name).expect("bus node"), format!("/{name}/reading"))
}

fn run(args: &[&str], domain: Option<&str>) -> (Output, Duration) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hypha"));
    command.arg("topic").arg("list").args(args);
    if let Some(domain) = domain {
        command.env("HYPHA_DOMAIN", domain);
    }
    let began = Instant::now();
    let output = command.output().expect("hypha topic list");
    (output, began.elapsed())
}

/// The lines of standard output that name `topic`.
fn lines_of(output: &Output, topic: &str) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.split_whitespace().next() == Some(topic))
        .map(str::to_owned)
        .collect()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The issue: a node whose topics are silent showed nothing.
#[test]
fn an_advertised_topic_that_has_not_published_is_listed_with_its_type() {
    let (bus, topic) = setup("tl_idle");
    let _idle = advertise(&bus, &topic, VERSION_1);

    let (output, _) = run(&[], None);

    assert!(output.status.success(), "{}", stderr_of(&output));
    assert_eq!(lines_of(&output, &topic), [format!("{topic}  [{DATA_TYPE}]")]);
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("no traffic"),
        "it does not listen any more"
    );
}

#[test]
fn a_topic_that_publishes_is_listed_as_well() {
    let (bus, topic) = setup("tl_active");
    let mut active = advertise(&bus, &topic, VERSION_1);
    assert!(active.publish(&[7, 0, 0, 0]));

    let (output, _) = run(&[], None);

    assert_eq!(lines_of(&output, &topic), [format!("{topic}  [{DATA_TYPE}]")]);
}

/// Two streams under one name are two lines, and only then does a line carry the MD5 start.
#[test]
fn two_streams_of_one_name_are_two_lines_with_the_start_of_their_md5() {
    let (bus, topic) = setup("tl_two");
    let _first = advertise(&bus, &topic, VERSION_1);
    let _second = advertise(&bus, &topic, VERSION_2);

    let (output, _) = run(&[], None);

    let mut expected: Vec<String> = [VERSION_1, VERSION_2]
        .iter()
        .map(|version| format!("{topic}  [{DATA_TYPE} ({})]", &md5_of(version)[..8]))
        .collect();
    let mut listed = lines_of(&output, &topic);
    expected.sort();
    listed.sort();
    assert_eq!(listed, expected);
}

#[test]
fn a_stream_without_a_definition_is_listed_by_its_schema_hash() {
    let (bus, topic) = setup("tl_bare");
    let md5 = "00112233445566778899aabbccddeeff";
    let _bare = bus.publish_raw(&topic, DATA_TYPE, md5, "").expect("a publisher");
    let hash = hypha_node::sdk::schema_hash_from_md5(md5).expect("hash");

    let (output, _) = run(&[], None);

    assert_eq!(lines_of(&output, &topic), [format!("{topic}  [? ({hash:016x})]")]);
}

#[test]
fn verbose_adds_the_publishers_and_the_subscribers_of_each_stream() {
    let (bus, topic) = setup("tl_verbose");
    let _publisher = advertise(&bus, &topic, VERSION_1);

    let (output, _) = run(&["-v"], None);

    assert_eq!(
        lines_of(&output, &topic),
        [format!("{topic}  [{DATA_TYPE}]  1 publisher, 0 subscribers")]
    );
}

/// A domain with nothing in it is an empty list, said so on standard error, and a success;
/// standard output stays empty for a script.
#[test]
fn an_empty_domain_lists_nothing_and_says_so() {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
    let domain = format!("t_{}_tl_{nonce:x}", std::process::id());

    let (output, took) = run(&[], Some(&domain));

    assert!(output.status.success(), "{}", stderr_of(&output));
    assert!(output.stdout.is_empty(), "{:?}", String::from_utf8_lossy(&output.stdout));
    assert!(stderr_of(&output).contains("no topics"), "{}", stderr_of(&output));
    assert!(took < Duration::from_secs(10), "it lists and returns: {took:?}");
}

/// There is nothing to wait for: the old flag is refused, not ignored.
#[test]
fn the_listening_time_is_gone() {
    let (output, _) = run(&["--timeout", "0.2"], None);

    assert!(!output.status.success());
    assert!(stderr_of(&output).contains("--timeout"), "{}", stderr_of(&output));
}
