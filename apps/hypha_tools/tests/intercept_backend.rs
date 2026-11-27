//! `hypha intercept` on the bus: a producer, the backend binary, and a consumer.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use hypha_node::sdk::message::DecodeOwned as _;
use hypha_node::sdk::{BusNode, PublisherOptions, RawPublisher, RawSubscriber, SubscriberOptions};
use hypha_testkit::unique_name as unique;
use hypha_tools::intercept_doc::{
    BackendState, InterceptConfig, InterceptionSpec, OverrideSpec, RelayState, state_topic,
    write_config_atomic,
};

const DATA_TYPE: &str = "intercept_test/Battery";
const DEFINITION: &str = "uint16 status\nstring serial_number\nintercept_test/Cell[] cells\n\
================================================================================\n\
MSG: intercept_test/Cell\nfloat32 voltage\n";
const OTHER_DEFINITION: &str = "uint32 status\n";
const LIMIT: Duration = Duration::from_secs(15);

fn battery(status: u16, serial: &str) -> Vec<u8> {
    let mut wire = status.to_le_bytes().to_vec();
    wire.extend_from_slice(&u32::try_from(serial.len()).unwrap().to_le_bytes());
    wire.extend_from_slice(serial.as_bytes());
    wire.extend_from_slice(&1_u32.to_le_bytes());
    wire.extend_from_slice(&3.5_f32.to_le_bytes());
    wire
}

/// Waits until `topic` is open on the bus, so a subscriber that declares no policy may join it.
fn await_topic(bus: &BusNode, topic: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let listed = bus.list_topics().unwrap_or_default();
        if listed.iter().any(|entry| entry.name == topic) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "no publisher opened '{topic}' in 20 s");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn producer(bus: &BusNode, topic: &str, definition: &str) -> RawPublisher {
    let md5 = hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, definition).expect("md5");
    bus.publish_raw(topic, DATA_TYPE, &md5, definition).expect("producer")
}

/// One test's topics, config file and backend process.
struct Bench {
    name: String,
    input: String,
    output: String,
    config: PathBuf,
    backend: Option<Child>,
    bus: BusNode,
    /// Holds the output topic open for the test's consumers.
    _output_opener: RawPublisher,
}

impl Bench {
    fn new(label: &str) -> Self {
        let name = unique(label);
        let dir = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let bus = BusNode::new(&format!("{name}_test")).expect("test node");
        let output = format!("/{name}/out");
        // The tool opens its output on the first frame, but a subscriber that declares
        // no policy must find the topic open. The bench opens it here.
        let output_md5 =
            hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, DEFINITION).expect("md5");
        let output_opener =
            bus.publish_raw(&output, DATA_TYPE, &output_md5, DEFINITION).expect("output opener");
        Self {
            input: format!("/{name}/raw"),
            output,
            config: dir.join("intercept.toml"),
            backend: None,
            bus,
            name,
            _output_opener: output_opener,
        }
    }

    fn write(&self, revision: u64, armed: bool, overrides: &[(&str, OverrideSpec)]) {
        let config = InterceptConfig {
            revision,
            interception: vec![InterceptionSpec {
                name: "battery".to_owned(),
                input: self.input.clone(),
                output: self.output.clone(),
                armed,
                overrides: overrides
                    .iter()
                    .map(|(path, spec)| ((*path).to_owned(), spec.clone()))
                    .collect::<BTreeMap<_, _>>(),
            }],
        };
        write_config_atomic(&self.config, &config).expect("config");
    }

    fn start(&mut self) {
        let child = Command::new(env!("CARGO_BIN_EXE_hypha"))
            .arg("intercept")
            .args(["--name", &self.name, "--config"])
            .arg(&self.config)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("hypha_intercept");
        self.backend = Some(child);
    }

    fn consumer(&self) -> RawSubscriber {
        let options = SubscriberOptions::default().keeping(8);
        self.bus.subscribe_raw_with_schema(&self.output, DATA_TYPE, "", options).expect("consumer")
    }

    fn state_subscriber(&self) -> RawSubscriber {
        // The tool publishes its state topic when it starts, so the subscriber waits for it.
        await_topic(&self.bus, &state_topic(&self.name));
        self.bus
            .subscribe_raw_with_schema(
                &state_topic(&self.name),
                "std_msgs/String",
                "",
                hypha_node::sdk::SubscriberOptions::default().keeping(4),
            )
            .expect("state subscriber")
    }

    /// Publishes `frame` every 20 ms until the consumer receives a frame `accept` takes.
    fn await_output(
        &self,
        publisher: &mut RawPublisher,
        consumer: &mut RawSubscriber,
        frame: &[u8],
        accept: impl Fn(&[u8]) -> bool,
    ) -> Vec<u8> {
        let deadline = Instant::now() + LIMIT;
        let mut last = None;
        while Instant::now() < deadline {
            assert!(publisher.publish(frame));
            std::thread::sleep(Duration::from_millis(20));
            while let Some(sample) = consumer.try_receive().expect("receive") {
                let bytes = sample.bytes().expect("payload").to_vec();
                if accept(&bytes) {
                    return bytes;
                }
                last = Some(bytes);
            }
        }
        let mut state = self.state_subscriber();
        std::thread::sleep(Duration::from_millis(100));
        let state = read_state(&mut state);
        panic!("no accepted output within {LIMIT:?}; last {last:?}; backend {state:?}");
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        if let Some(mut child) = self.backend.take() {
            let pid = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap());
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(dir) = self.config.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn read_state(subscriber: &mut RawSubscriber) -> Option<BackendState> {
    let mut newest = None;
    while let Some(sample) = subscriber.try_receive().expect("state receive") {
        let message =
            hypha_node::sdk::msgs::std_msgs::String::decode_from(sample.bytes().expect("payload"))
                .expect("std_msgs/String");
        newest = Some(toml::from_str::<BackendState>(&message.data).expect("state TOML"));
    }
    newest
}

fn await_state(
    subscriber: &mut RawSubscriber,
    publisher: Option<&mut RawPublisher>,
    accept: impl Fn(&BackendState) -> bool,
) -> BackendState {
    let deadline = Instant::now() + LIMIT;
    let mut last = None;
    let mut publisher = publisher;
    while Instant::now() < deadline {
        if let Some(publisher) = publisher.as_deref_mut() {
            assert!(publisher.publish(&battery(1, "AB")));
        }
        if let Some(state) = read_state(subscriber) {
            if accept(&state) {
                return state;
            }
            last = Some(state);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("no accepted state within {LIMIT:?}; last {last:?}");
}

fn bits(set: &str) -> OverrideSpec {
    OverrideSpec::Bits { set: set.to_owned(), clear: "0".to_owned() }
}

fn status(frame: &[u8]) -> u16 {
    u16::from_le_bytes([frame[0], frame[1]])
}

#[test]
fn armed_override_reaches_the_consumer() {
    let mut bench = Bench::new("icpt_armed");
    bench.write(1, true, &[("status", bits("0x0002"))]);
    let mut publisher = producer(&bench.bus, &bench.input, DEFINITION);
    let mut consumer = bench.consumer();
    bench.start();
    let out = bench.await_output(&mut publisher, &mut consumer, &battery(1, "AB"), |_| true);
    assert_eq!(out, battery(3, "AB"));

    let mut state = bench.state_subscriber();
    let state = await_state(&mut state, None, |state| {
        state.interception.first().is_some_and(|entry| entry.published > 0)
    });
    let entry = &state.interception[0];
    assert_eq!(entry.state, RelayState::Live);
    assert_eq!(entry.data_type, DATA_TYPE);
    assert!(entry.modified > 0);

    let hash = hypha_node::sdk::schema_hash_from_md5(
        &hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, DEFINITION).unwrap(),
    )
    .unwrap();
    let identity = bench.bus.resolve_schema_hash(&bench.output, hash).expect("resolve");
    assert_eq!(identity.map(|schema| schema.data_type), Some(DATA_TYPE.to_owned()));
}

#[test]
fn bypass_forwards_unchanged() {
    let mut bench = Bench::new("icpt_bypass");
    bench.write(1, false, &[("status", bits("0x0002"))]);
    let mut publisher = producer(&bench.bus, &bench.input, DEFINITION);
    let mut consumer = bench.consumer();
    bench.start();
    let out = bench.await_output(&mut publisher, &mut consumer, &battery(1, "AB"), |_| true);
    assert_eq!(out, battery(1, "AB"));
}

#[test]
fn relay_publishes_a_string_longer_than_the_input() {
    let mut bench = Bench::new("icpt_grow");
    let long = "S".repeat(64);
    bench.write(1, true, &[("serial_number", OverrideSpec::Value { value: long.clone() })]);
    let mut publisher = producer(&bench.bus, &bench.input, DEFINITION);
    let mut consumer = bench.consumer();
    bench.start();
    let out = bench.await_output(&mut publisher, &mut consumer, &battery(1, "AB"), |_| true);
    assert_eq!(out, battery(1, &long));
}

#[test]
fn config_reload_applies_the_new_revision() {
    let mut bench = Bench::new("icpt_reload");
    bench.write(1, true, &[("status", bits("0x0002"))]);
    let mut publisher = producer(&bench.bus, &bench.input, DEFINITION);
    let mut consumer = bench.consumer();
    bench.start();
    bench.await_output(&mut publisher, &mut consumer, &battery(1, "AB"), |out| status(out) == 3);

    bench.write(2, true, &[("status", bits("0x0004"))]);
    let mut state = bench.state_subscriber();
    await_state(&mut state, None, |state| state.applied_revision == 2);
    bench.await_output(&mut publisher, &mut consumer, &battery(1, "AB"), |out| status(out) == 5);
}

#[test]
fn changing_the_output_moves_the_relay() {
    let mut bench = Bench::new("icpt_move");
    bench.write(1, true, &[("status", bits("0x0002"))]);
    let mut publisher = producer(&bench.bus, &bench.input, DEFINITION);
    let mut old_consumer = bench.consumer();
    bench.start();
    bench.await_output(&mut publisher, &mut old_consumer, &battery(1, "AB"), |_| true);

    bench.output = format!("{}_moved", bench.output);
    // The renamed output is a new topic: the test opens it before its consumer joins.
    let moved_md5 = hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, DEFINITION).expect("md5");
    let _moved_opener = bench
        .bus
        .publish_raw(&bench.output, DATA_TYPE, &moved_md5, DEFINITION)
        .expect("the moved output's opener");
    let mut new_consumer = bench.consumer();
    bench.write(2, true, &[("status", bits("0x0002"))]);
    bench.await_output(&mut publisher, &mut new_consumer, &battery(1, "AB"), |_| true);
    while old_consumer.try_receive().expect("receive").is_some() {}
    assert!(publisher.publish(&battery(1, "AB")));
    std::thread::sleep(Duration::from_millis(200));
    assert!(old_consumer.try_receive().expect("receive").is_none(), "the old output is stopped");
}

#[test]
fn invalid_config_keeps_last_state_and_reports() {
    let mut bench = Bench::new("icpt_invalid");
    bench.write(1, true, &[("status", bits("0x0002"))]);
    let mut publisher = producer(&bench.bus, &bench.input, DEFINITION);
    let mut consumer = bench.consumer();
    bench.start();
    bench.await_output(&mut publisher, &mut consumer, &battery(1, "AB"), |out| status(out) == 3);

    std::fs::write(&bench.config, "revision = [").expect("broken config");
    let mut state = bench.state_subscriber();
    let state = await_state(&mut state, None, |state| !state.config_error.is_empty());
    assert_eq!(state.applied_revision, 1);
    bench.await_output(&mut publisher, &mut consumer, &battery(8, "AB"), |out| status(out) == 10);
}

#[test]
fn state_topic_is_latched() {
    let mut bench = Bench::new("icpt_latched");
    bench.write(4, true, &[]);
    bench.start();
    let mut early = bench.state_subscriber();
    await_state(&mut early, None, |state| state.applied_revision == 4);
    let config = std::path::absolute(&bench.config).expect("config path");

    let mut late = bench.state_subscriber();
    let deadline = Instant::now() + Duration::from_millis(200);
    let mut state = None;
    while state.is_none() && Instant::now() < deadline {
        state = read_state(&mut late);
        std::thread::sleep(Duration::from_millis(5));
    }
    let state = state.expect("a late subscriber receives the latched state at once");
    assert_eq!(state.backend, bench.name);
    assert_eq!(Path::new(&state.config_path), config.as_path());
}

#[test]
fn schema_change_reports_on_the_state_topic() {
    let mut bench = Bench::new("icpt_schema");
    bench.write(1, true, &[("status", bits("0x0002"))]);
    let mut publisher = producer(&bench.bus, &bench.input, DEFINITION);
    let mut consumer = bench.consumer();
    bench.start();
    bench.await_output(&mut publisher, &mut consumer, &battery(1, "AB"), |_| true);
    drop(publisher);

    let other_bus = BusNode::new(&format!("{}_other", bench.name)).expect("other node");
    let md5 = hypha_node::sdk::md5::md5_from_definition(DATA_TYPE, OTHER_DEFINITION).expect("md5");
    let options = PublisherOptions::default();
    let mut other = other_bus
        .publish_raw_configured(&bench.input, DATA_TYPE, &md5, OTHER_DEFINITION, options)
        .expect("restarted producer");
    let mut state = bench.state_subscriber();
    let deadline = Instant::now() + LIMIT;
    let mut changed = None;
    while changed.is_none() && Instant::now() < deadline {
        assert!(other.publish(&7_u32.to_le_bytes()));
        changed = read_state(&mut state)
            .filter(|state| state.interception[0].state == RelayState::SchemaChanged);
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(changed.is_some(), "the state reports schema_changed");
    while consumer.try_receive().expect("receive").is_some() {}
    assert!(other.publish(&7_u32.to_le_bytes()));
    std::thread::sleep(Duration::from_millis(200));
    assert!(consumer.try_receive().expect("receive").is_none(), "nothing forwarded");

    // Saving the interception again rebuilds it on the producer's new schema, which is a
    // stream of its own on the output too.
    let _new_output = bench
        .bus
        .publish_raw(&bench.output, DATA_TYPE, &md5, OTHER_DEFINITION)
        .expect("the new output's opener");
    let options = SubscriberOptions::default().keeping(8);
    let mut consumer = bench
        .bus
        .subscribe_raw_with_schema(&bench.output, DATA_TYPE, &md5, options)
        .expect("a consumer of the new schema");
    bench.write(2, true, &[("status", bits("0x0002"))]);
    let out = bench.await_output(&mut other, &mut consumer, &5_u32.to_le_bytes(), |_| true);
    assert_eq!(out, 7_u32.to_le_bytes());
}
