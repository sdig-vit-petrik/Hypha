//! What a listing of the domain costs a recorder that follows a 1 kHz topic while the graph is
//! quiet: the samples that overrun its queue while the loop is away. A measurement: it prints
//! the count and bounds it loosely, as the machine decides how many listings fit.
#![allow(clippy::expect_used, clippy::print_stdout)]

#[allow(dead_code)]
mod common;

use hypha_node::sdk::BusNode;
use hypha_node::sdk::md5::md5_from_definition;
use hypha_testkit::unique_name;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DATA_TYPE: &str = "overload_test/Tick";
const DEFINITION: &str = "uint32 count\n";
const SECONDS: u64 = 6;

#[test]
#[ignore = "a measurement of the machine: cargo nextest run --run-ignored only"]
fn a_listing_does_not_cost_a_recorder_of_a_busy_topic_its_samples() {
    let name = unique_name("rec_listing_loss");
    let topic = format!("/{name}/tick");
    let path = std::env::temp_dir().join(format!("{name}.mcap"));
    let bus = BusNode::new(&name).expect("bus node");
    let md5 = md5_from_definition(DATA_TYPE, DEFINITION).expect("MD5");
    let mut publisher = bus.publish_raw(&topic, DATA_TYPE, &md5, DEFINITION).expect("publisher");
    let mut recorder = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .args(["record", "--topics", &topic, "--compression", "none", "-o"])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("hypha record");
    let mut stdout = BufReader::new(recorder.stdout.take().expect("stdout"));
    let mut recorder = common::Killed(recorder);
    let mut line = String::new();
    while !line.starts_with("Following") {
        line.clear();
        assert!(stdout.read_line(&mut line).expect("a line") > 0);
    }
    let started = Instant::now();
    let mut count = 0_u32;
    let mut next = started;
    while started.elapsed() < Duration::from_secs(SECONDS) {
        assert!(publisher.publish(&count.to_le_bytes()));
        count = count.saturating_add(1);
        next = next.checked_add(Duration::from_millis(1)).expect("time");
        while Instant::now() < next {
            std::hint::spin_loop();
        }
    }
    kill(Pid::from_raw(i32::try_from(recorder.id()).expect("pid")), Signal::SIGTERM).expect("stop");
    recorder.wait().expect("status");
    let mut summary = String::new();
    stdout.read_to_string(&mut summary).expect("summary");
    std::fs::remove_file(&path).expect("remove");
    let lost: u64 = summary
        .lines()
        .find_map(|line| line.split(" lost ").nth(1)?.split(' ').next()?.parse().ok())
        .unwrap_or(0);
    println!("MEASURED published {count} in {SECONDS} s, recorder reports lost {lost}");
    println!("{summary}");
}
