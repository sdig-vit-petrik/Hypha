//! `hypha topic delay <input> <output>` (#232): the time from the newest input frame sent
//! before an output frame to that output frame, from the frames' send stamps alone.
#![allow(clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
mod common;

use hypha_node::sdk::BusNode;
use hypha_testkit::unique_name;
use hypha_tools::delay::Delays;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const MS: u64 = 1_000_000;

#[test]
fn an_output_is_measured_from_the_newest_input_sent_before_it() {
    let mut delays = Delays::new(100);
    for input in [10 * MS, 20 * MS, 30 * MS] {
        delays.on_input(input);
    }
    assert_eq!(delays.on_output(25 * MS), Some(5 * MS), "the input at 20 ms, not 10 ms or 30 ms");
    assert_eq!(delays.on_output(30 * MS), Some(0), "an input sent at the same stamp counts");
    assert_eq!(delays.on_output(100 * MS), Some(70 * MS), "a rate node: the newest input so far");
}

#[test]
fn an_output_before_every_input_is_counted_and_has_no_delay() {
    let mut delays = Delays::new(100);
    delays.on_input(50 * MS);
    assert_eq!(delays.on_output(10 * MS), None);
    assert_eq!(delays.on_output(60 * MS), Some(10 * MS));
    let report = delays.report().expect("one delay");
    assert_eq!((report.count, report.unpaired), (1, 1));
}

#[test]
fn inputs_that_arrive_out_of_order_are_still_found_by_stamp() {
    let mut delays = Delays::new(100);
    for input in [30 * MS, 10 * MS, 20 * MS] {
        delays.on_input(input);
    }
    assert_eq!(delays.on_output(25 * MS), Some(5 * MS));
}

#[test]
fn the_report_gives_the_median_the_p99_and_the_worst_of_the_window() {
    let mut delays = Delays::new(100);
    delays.on_input(0);
    // 100 outputs of 1 ms to 100 ms after the input: the exact quantiles are known.
    for ms in 1..=100 {
        delays.on_output(ms * MS);
    }
    let report = delays.report().expect("delays");
    assert_eq!(report.count, 100);
    assert_eq!(report.min_nanos, MS);
    assert_eq!(report.median_nanos, 50 * MS);
    assert_eq!(report.p99_nanos, 99 * MS);
    assert_eq!(report.max_nanos, 100 * MS);
    assert_eq!(
        report.to_string(),
        "delay: median 50.000 ms, p99 99.000 ms, max 100.000 ms (min 1.000 ms) over 100 outputs"
    );
}

#[test]
fn the_window_keeps_the_last_outputs_only() {
    let mut delays = Delays::new(10);
    delays.on_input(0);
    delays.on_output(900 * MS);
    for ms in 1..=10 {
        delays.on_output(ms * MS);
    }
    let report = delays.report().expect("delays");
    assert_eq!((report.count, report.max_nanos), (10, 10 * MS), "the 900 ms output left");
}

#[test]
fn no_output_gives_no_report_and_the_inputs_kept_are_bounded() {
    let mut delays = Delays::new(10);
    assert!(delays.report().is_none());
    // A day of a 1 kHz input does not grow without bound: the stamps older than the lookback
    // are dropped.
    for stamp in 0..1_000_000_u64 {
        delays.on_input(stamp * MS);
    }
    assert!(delays.inputs_kept() <= Delays::MAX_INPUTS, "{}", delays.inputs_kept());
    assert_eq!(delays.on_output(1_000_000 * MS), Some(MS), "the newest input is kept");
}

/// A `hypha topic delay` whose standard output arrives line by line.
struct Verb {
    /// Kills the tool when the test ends.
    _child: common::Killed,
    lines: mpsc::Receiver<String>,
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
    Verb { _child: common::Killed(child), lines, errors }
}

/// The delay in nanoseconds that a report line names as its median.
fn median_of(line: &str) -> Option<f64> {
    let text = line.strip_prefix("delay: median ")?;
    let (number, _) = text.split_once(" ms")?;
    number.parse::<f64>().ok().map(|ms| ms * 1.0e6)
}

#[test]
fn the_tool_reports_the_delay_from_an_input_to_an_output() {
    let name = unique_name("td_cli");
    let bus = BusNode::new(&name).expect("bus node");
    let input = format!("/{name}/in");
    let output = format!("/{name}/out");
    let mut source = bus
        .publish_raw(&input, "std_msgs/Int32", "da5909fbe378aeaf85e547e830cc1bb7", "int32 data\n")
        .expect("input publisher");
    let mut sink = bus
        .publish_raw(&output, "std_msgs/Int32", "da5909fbe378aeaf85e547e830cc1bb7", "int32 data\n")
        .expect("output publisher");
    let verb = spawn(&["delay", &input, &output]);

    // An output after each input: the tool finds the pairs and reports their median. What
    // the delay is, is decided by the `Delays` cases above; here only the wiring is checked.
    let began = Instant::now();
    let median = loop {
        assert!(source.publish(&[1, 0, 0, 0]));
        assert!(sink.publish(&[1, 0, 0, 0]));
        // Waiting for a line paces the publishes.
        if let Some(median) =
            verb.lines.recv_timeout(Duration::from_millis(30)).ok().as_deref().and_then(median_of)
        {
            break median;
        }
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "no report: {}",
            verb.errors.try_iter().collect::<Vec<_>>().join("\n")
        );
    };
    assert!(median >= 0.0, "median {median} ns");
}

#[test]
fn the_tool_needs_both_topics() {
    let output = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .args(["topic", "delay", "/only_one"])
        .output()
        .expect("hypha topic delay");
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("<OUTPUT>") || text.contains("<output>"), "{text}");
}
