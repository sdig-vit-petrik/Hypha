//! A recording is evidence: a stream that joins a followed name after the recording began is
//! recorded from the moment the recorder followed it, and the recorder says how many samples the
//! stream had sent before (H1 of the fourth review of #268).
#![allow(clippy::expect_used, clippy::print_stdout, clippy::panic)]

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
const VERSION_1: &str = "uint32 count\n";
const VERSION_2: &str = "uint32 count\nuint8 pad\n";

/// The first counter the recording holds for the stream of `md5`.
fn first_recorded(path: &std::path::Path, md5: &str) -> u32 {
    let bytes = std::fs::read(path).expect("the recording");
    mcap::MessageStream::new(&bytes)
        .expect("an MCAP reader")
        .map(|message| message.expect("a valid message"))
        .find(|message| message.channel.metadata.get("md5sum").is_some_and(|found| found == md5))
        .map(|message| {
            u32::from_le_bytes(message.data.first_chunk::<4>().copied().expect("four bytes"))
        })
        .expect("a message of the stream")
}

/// Paces `rate` frames a second for `seconds`, the counter first.
fn pace(publisher: &mut hypha_node::sdk::RawPublisher, rate: u32, seconds: u64, pad: bool) {
    let started = Instant::now();
    let period = Duration::from_secs(1).checked_div(rate).expect("a rate");
    let mut next = started;
    let mut count = 0_u32;
    while started.elapsed() < Duration::from_secs(seconds) {
        let mut bytes = count.to_le_bytes().to_vec();
        if pad {
            bytes.push(0);
        }
        assert!(publisher.publish(&bytes));
        count = count.saturating_add(1);
        next = next.checked_add(period).expect("time");
        while Instant::now() < next {
            std::hint::spin_loop();
        }
    }
}

/// Records a name that has version 1 (10 Hz, from before the recorder) while version 2 joins at
/// `rate` after the recorder follows. Returns what the recorder reported for version 2, what its
/// summary says, and the first counter it recorded of version 2.
fn run(rate: u32) -> (u64, String, u32) {
    let name = unique_name("rec_late");
    let topic = format!("/{name}/tick");
    let path = std::env::temp_dir().join(format!("{name}.mcap"));
    let bus = BusNode::new(&name).expect("bus node");
    let (md5_1, md5_2) = (
        md5_from_definition(DATA_TYPE, VERSION_1).expect("MD5"),
        md5_from_definition(DATA_TYPE, VERSION_2).expect("MD5"),
    );
    let mut v1 = bus.publish_raw(&topic, DATA_TYPE, &md5_1, VERSION_1).expect("v1");
    let mut recorder = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .args(["record", "--topics", &topic, "--compression", "none", "-o"])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hypha record");
    let mut stdout = BufReader::new(recorder.stdout.take().expect("stdout"));
    let stderr = recorder.stderr.take().expect("stderr");
    let mut recorder = common::Killed(recorder);
    let mut line = String::new();
    while !line.starts_with("Following 1 stream") {
        line.clear();
        assert!(stdout.read_line(&mut line).expect("a line") > 0, "the recorder ended early");
    }
    let told = std::thread::spawn(move || {
        BufReader::new(stderr).lines().map_while(Result::ok).collect::<Vec<_>>()
    });
    pace(&mut v1, 10, 1, false);
    let mut v2 = bus.publish_raw(&topic, DATA_TYPE, &md5_2, VERSION_2).expect("v2");
    pace(&mut v2, rate, 3, true);
    kill(Pid::from_raw(i32::try_from(recorder.id()).expect("pid")), Signal::SIGTERM).expect("stop");
    recorder.wait().expect("status");
    let mut summary = String::new();
    stdout.read_to_string(&mut summary).expect("summary");
    let told = told.join().expect("stderr").join("\n");
    let first = first_recorded(&path, &md5_2);
    std::fs::remove_file(&path).expect("remove");
    let reported = summary
        .lines()
        .find_map(|line| {
            let line = line.trim();
            line.contains("sent ")
                .then(|| line.split("sent ").nth(1)?.split(' ').next()?.parse().ok())?
        })
        .unwrap_or(0);
    println!(
        "{rate} Hz: the recorder reports {reported} sent before following; it recorded from counter {first}"
    );
    println!("{summary}\n{told}");
    (reported, summary, first)
}

#[test]
fn a_stream_that_joins_at_100_hz_says_how_many_samples_it_sent_before_it_was_followed() {
    let (reported, summary, first) = run(100);
    assert!(summary.contains("samples before it was followed"), "{summary}");
    assert_eq!(
        reported,
        u64::from(first),
        "the counter starts at 0, so the first recorded is the count"
    );
    assert!(
        !summary.contains("overload_test/Tick (")
            || summary.matches("before it was followed").count() == 1,
        "only the stream that came late: {summary}"
    );
}

#[test]
fn a_stream_that_joins_at_1_khz_says_how_many_samples_it_sent_before_it_was_followed() {
    let (reported, _summary, first) = run(1000);
    assert_eq!(reported, u64::from(first));
}
