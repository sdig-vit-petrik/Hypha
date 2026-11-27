//! `hypha topic echo`, `hz` and `bw` read every stream of a topic name (#209): each stream is
//! labelled with its type and MD5 start, each is measured on its own, a stream that joins
//! later is followed, and `--type`/`--md5` choose among them.
#![allow(clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
mod common;

use hypha_node::sdk::md5::md5_from_definition;
use hypha_node::sdk::{BusNode, RawPublisher};
use hypha_testkit::unique_name;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const DATA_TYPE: &str = "overload_test/Reading";
const VERSION_1: &str = "int32 value\n";
const VERSION_2: &str = "int32 value\nstring label\nfloat64 weight\n";
const LIMIT: Duration = Duration::from_secs(20);

fn md5_of(definition: &str) -> String {
    md5_from_definition(DATA_TYPE, definition).expect("MD5")
}

fn label_of(definition: &str) -> String {
    format!("{DATA_TYPE} ({})", &md5_of(definition)[..8])
}

/// A `hypha topic` verb whose standard output arrives line by line.
struct Verb {
    child: common::Killed,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
    errors: mpsc::Receiver<String>,
}

fn spawn(args: &[&str]) -> Verb {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .arg("topic")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha topic");
    let (sender, lines) = mpsc::channel();
    let (error_sender, errors) = mpsc::channel();
    for (stream, sender) in [
        (Box::new(child.stdout.take().expect("stdout")) as Box<dyn std::io::Read + Send>, sender),
        (
            Box::new(child.stderr.take().expect("stderr")) as Box<dyn std::io::Read + Send>,
            error_sender,
        ),
    ] {
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
    }
    Verb { child: common::Killed(child), lines, seen: Vec::new(), errors }
}

impl Verb {
    /// Takes the lines that arrived since the last call, into `seen`.
    fn collect(&mut self) {
        while let Ok(line) = self.lines.try_recv() {
            self.seen.push(line);
        }
    }

    /// What the verb said on standard error, for the message of a failed assertion.
    fn errors(&self) -> String {
        self.errors.try_iter().collect::<Vec<_>>().join("\n")
    }

    fn exited(&mut self) -> bool {
        self.child.try_wait().expect("status").is_some()
    }
}

/// One publisher for each version, and the message each sends: version 1 an int32, version 2
/// an int32, a string and a float64.
struct Versions {
    first: RawPublisher,
    second: Option<RawPublisher>,
    topic: String,
}

impl Versions {
    fn publish_first(bus: &BusNode, topic: &str) -> Self {
        let first = bus.publish_raw(topic, DATA_TYPE, &md5_of(VERSION_1), VERSION_1).expect("v1");
        Self { first, second: None, topic: topic.to_owned() }
    }

    fn join_second(&mut self, bus: &BusNode) {
        let md5 = md5_of(VERSION_2);
        self.second = Some(bus.publish_raw(&self.topic, DATA_TYPE, &md5, VERSION_2).expect("v2"));
    }

    /// One message of each version that exists.
    fn send(&mut self) {
        assert!(self.first.publish(&[7, 0, 0, 0]));
        if let Some(second) = self.second.as_mut() {
            let mut bytes = vec![9, 0, 0, 0, 2, 0, 0, 0, b'a', b'b'];
            bytes.extend_from_slice(&1.5_f64.to_le_bytes());
            assert!(second.publish(&bytes));
        }
    }
}

/// Publishes until `done` says so for the lines the verb printed, or the limit passes.
fn until(verb: &mut Verb, versions: &mut Versions, done: impl Fn(&[String]) -> bool) {
    let began = Instant::now();
    loop {
        versions.send();
        std::thread::sleep(Duration::from_millis(40));
        verb.collect();
        if done(&verb.seen) || verb.exited() {
            verb.collect();
            return;
        }
        assert!(began.elapsed() < LIMIT, "the verb did not finish: {:?}", verb.seen);
    }
}

fn setup(label: &str) -> (BusNode, String) {
    let name = unique_name(label);
    (BusNode::new(&name).expect("bus node"), format!("/{name}/reading"))
}

fn has(seen: &[String], text: &str) -> bool {
    seen.iter().any(|line| line.contains(text))
}

#[test]
fn echo_prints_every_stream_of_the_name_each_under_its_label() {
    let (bus, topic) = setup("ts_echo");
    let mut versions = Versions::publish_first(&bus, &topic);
    versions.join_second(&bus);
    let mut echo = spawn(&["echo", &topic, "--count", "6"]);

    until(&mut echo, &mut versions, |_| false);

    let text = echo.seen.join("\n");
    println!("{text}");
    let errors = echo.errors();
    assert!(has(&echo.seen, &format!("# {}", label_of(VERSION_1))), "{text} // {errors}");
    assert!(has(&echo.seen, &format!("# {}", label_of(VERSION_2))), "{text} // {errors}");
    assert!(has(&echo.seen, "weight"), "the second version's fields are decoded: {text}");
    assert!(echo.child.wait().expect("status").success());
}

#[test]
fn echo_p_prints_the_header_of_a_stream_before_each_run_of_its_rows() {
    let (bus, topic) = setup("ts_plot");
    let mut versions = Versions::publish_first(&bus, &topic);
    versions.join_second(&bus);
    let mut echo = spawn(&["echo", "-p", &topic, "--count", "6"]);

    until(&mut echo, &mut versions, |_| false);

    let text = echo.seen.join("\n");
    println!("{text}");
    // Every row has as many columns as the header above it.
    let mut columns = None;
    let mut rows = 0_usize;
    for line in &echo.seen {
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("%time") {
            columns = Some(line.split(',').count());
        } else {
            assert_eq!(columns, Some(line.split(',').count()), "a misaligned row: {text}");
            rows += 1;
        }
    }
    assert_eq!(rows, 6, "{text}");
    assert!(has(&echo.seen, "%time,value,label,weight"), "{text}");
    assert!(has(&echo.seen, &format!("# {}", label_of(VERSION_2))), "{text}");
}

#[test]
fn a_selector_follows_one_stream_and_prints_it_as_a_topic_of_one() {
    let (bus, topic) = setup("ts_select");
    let mut versions = Versions::publish_first(&bus, &topic);
    versions.join_second(&bus);
    let prefix = md5_of(VERSION_2)[..6].to_owned();
    let mut echo = spawn(&["echo", "-p", &topic, "--md5", &prefix, "--count", "3"]);

    until(&mut echo, &mut versions, |_| false);

    assert_eq!(echo.seen.iter().filter(|line| line.starts_with("%time")).count(), 1);
    assert!(!has(&echo.seen, "#"), "a lone stream prints no label: {:?}", echo.seen);
    assert!(has(&echo.seen, "%time,value,label,weight"), "{:?}", echo.seen);
}

#[test]
fn echo_started_before_any_publisher_waits_and_follows_streams_as_they_join() {
    let (bus, topic) = setup("ts_late");
    let mut echo = spawn(&["echo", &topic]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(!echo.exited(), "it waits for the topic");

    let mut versions = Versions::publish_first(&bus, &topic);
    until(&mut echo, &mut versions, |seen| has(seen, "value: 7"));
    assert!(!has(&echo.seen, "# "), "one stream: no label: {:?}", echo.seen);

    // The migration: the second version joins a name that is already being echoed. The
    // listing that finds it takes as long as the domain makes it, so the test waits for both.
    versions.join_second(&bus);
    let both = [format!("# {}", label_of(VERSION_1)), format!("# {}", label_of(VERSION_2))];
    until(&mut echo, &mut versions, |seen| both.iter().all(|label| has(seen, label)));
    let text = echo.seen.join("\n");
    println!("{text}");
    for label in &both {
        assert!(has(&echo.seen, label), "{text} // {}", echo.errors());
    }
}

#[test]
fn hz_and_bw_report_each_stream_on_its_own() {
    for verb in ["hz", "bw"] {
        let (bus, topic) = setup(&format!("ts_{verb}"));
        let mut versions = Versions::publish_first(&bus, &topic);
        versions.join_second(&bus);
        let mut measure = spawn(&[verb, &topic]);
        let titles = [
            format!("{topic} [{}]:", label_of(VERSION_1)),
            format!("{topic} [{}]:", label_of(VERSION_2)),
        ];

        until(&mut measure, &mut versions, |seen| titles.iter().all(|title| has(seen, title)));

        let text = measure.seen.join("\n");
        println!("{text}");
        for title in &titles {
            assert!(has(&measure.seen, title), "{verb}: {text}");
        }
        let figure = if verb == "hz" { "average rate" } else { "average:" };
        assert!(has(&measure.seen, figure), "{verb}: {text}");
    }
}
