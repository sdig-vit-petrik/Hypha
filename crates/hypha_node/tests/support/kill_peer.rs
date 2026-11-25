//! Test-only peer process of `tests/peer_kill_isolation.rs`. The test kills it with
//! SIGKILL, so it never ends on its own.
//!
//! ```text
//! hypha_node_kill_peer flood <node> <topic> <tag>   publish for ever, 5000 frames a second
//! hypha_node_kill_peer read  <node> <topic>         read every sample and release it
//! hypha_node_kill_peer hold  <node> <topic>         take two samples and keep them
//! hypha_node_kill_peer loan  <node> <topic> <tag>   publish a few frames, then keep a loan unsent
//! hypha_node_kill_peer step  <node> <topic> <tag>   send both outputs of a few steps, then one of two
//! ```
//!
//! It prints `ready` when its endpoint exists. `hold` then prints `holding` when it
//! keeps two samples, the most a subscriber may borrow. A frame of `flood` is nine
//! bytes: the tag, then a counter from zero, little endian.
//!
//! `loan` publishes [`FRAMES_BEFORE_THE_LOAN`] such frames, takes a loan for the next
//! counter and keeps it for ever, publishes one frame with the counter after that, and
//! prints `loaned`.
//!
//! `step` is a cycle with two outputs, `<topic>/first` and `<topic>/second`, as the
//! bus sees it: a step borrows a frame for each output, then sends them in order, both
//! stamped with the step's time. Its frames carry the step number as their counter. It
//! waits for [`STEP_READERS`] subscribers on each output, sends [`COMPLETE_STEPS`] whole steps, sends
//! the first output of one more step, prints `between` and keeps the second output's
//! frame unsent for ever. A real cycle also gives both frames of a step one transport
//! send stamp; here each send reads the clock, which no test of this peer looks at.

#![allow(clippy::expect_used, reason = "a failed setup ends the peer, and the test reports it")]

use std::io::Write as _;
use std::time::Duration;

use hypha_node::safe;
use hypha_node::sdk::{BusNode, PayloadCapacity, ReceivedSample};

const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const FLOOD_PERIOD: Duration = Duration::from_micros(200);
const POLL_PERIOD: Duration = Duration::from_micros(100);
const HELD_SAMPLES: usize = 2;
const FRAME_BYTES: usize = 9;
/// Frames `loan` publishes before the loan it keeps.
const FRAMES_BEFORE_THE_LOAN: u64 = 10;
/// Whole steps `step` sends before the step it leaves half sent.
const COMPLETE_STEPS: u64 = 3;
/// The framework time of step zero of `step`: 2026-10-08T12:00:00Z, in nanoseconds.
const FIRST_STEP_NANOS: u64 = 1_791_460_800_000_000_000;
const STEP_PERIOD_NANOS: u64 = 10_000_000;
/// The subscribers `step` waits for on each output: the test's cycle and its plain reader.
const STEP_READERS: usize = 2;

fn say(line: &str) {
    println!("{line}");
    std::io::stdout().flush().expect("flush stdout");
}

fn sleep_for_ever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn flood(bus: &BusNode, topic: &str, tag: u8) -> ! {
    let mut publisher = bus.publish_raw(topic, "test/Bytes", MD5, "").expect("publisher");
    say("ready");
    let mut counter = 0_u64;
    loop {
        let mut frame = [tag; 9];
        for (target, source) in frame.iter_mut().skip(1).zip(counter.to_le_bytes()) {
            *target = source;
        }
        // A refused publish is the test's finding to make, on its own side of the bus.
        let _sent = publisher.publish(&frame);
        counter = counter.wrapping_add(1);
        std::thread::sleep(FLOOD_PERIOD);
    }
}

fn read(bus: &BusNode, topic: &str) -> ! {
    let mut subscriber = bus.subscribe_raw(topic, 0).expect("subscriber");
    say("ready");
    loop {
        let _released = subscriber.try_receive().expect("receive");
        std::thread::sleep(POLL_PERIOD);
    }
}

fn hold(bus: &BusNode, topic: &str) -> ! {
    let mut subscriber = bus.subscribe_raw(topic, 0).expect("subscriber");
    say("ready");
    let mut held: Vec<ReceivedSample> = Vec::with_capacity(HELD_SAMPLES);
    while held.len() < HELD_SAMPLES {
        match subscriber.try_receive().expect("receive") {
            Some(sample) => held.push(sample),
            None => std::thread::sleep(POLL_PERIOD),
        }
    }
    say("holding");
    sleep_for_ever()
}

/// The node and a publisher of nine-byte frames on `topic`, through the functions
/// behind the C ABI: they lend a frame without sending it, which the SDK does not.
fn frame_publisher(node: &safe::HyphaNode, topic: &str) -> safe::HyphaPublisher {
    let settings = safe::PublisherSettings {
        latch: false,
        loaned_samples: 0,
        declaration: safe::Declaration::undeclared(PayloadCapacity::Growable),
    };
    safe::publish_with_qos(node, topic, "test/Bytes", MD5, "", settings).expect("publisher")
}

/// Borrows a frame of `publisher` and writes `tag` and `counter` into it.
fn borrow_frame(publisher: &safe::HyphaPublisher, tag: u8, counter: u64) -> safe::HyphaLoan {
    let mut loan = safe::publisher_loan_frame(publisher, FRAME_BYTES).expect("loan");
    let payload = loan.payload_mut().expect("a loan has a payload");
    payload.fill(tag);
    for (target, source) in payload.iter_mut().skip(1).zip(counter.to_le_bytes()) {
        *target = source;
    }
    loan
}

fn loan(node: &safe::HyphaNode, topic: &str, tag: u8) -> ! {
    let publisher = frame_publisher(node, topic);
    say("ready");
    for counter in 0..FRAMES_BEFORE_THE_LOAN {
        // Stamped with the framework clock, as `flood` stamps its frames.
        safe::loan_send(borrow_frame(&publisher, tag, counter), -1).expect("send");
        std::thread::sleep(FLOOD_PERIOD);
    }
    let _unsent = borrow_frame(&publisher, tag, FRAMES_BEFORE_THE_LOAN);
    // A frame after the loan: a reader sees a gap here if the loan took a sequence number.
    let after = FRAMES_BEFORE_THE_LOAN.saturating_add(1);
    safe::loan_send(borrow_frame(&publisher, tag, after), -1).expect("send");
    say("loaned");
    sleep_for_ever()
}

fn step(node: &safe::HyphaNode, topic: &str, tag: u8) -> ! {
    let first = frame_publisher(node, &format!("{topic}/first"));
    let second = frame_publisher(node, &format!("{topic}/second"));
    say("ready");
    let has_readers = |output: &safe::HyphaPublisher| {
        safe::publisher_subscriber_count(output) >= Some(STEP_READERS)
    };
    while !(has_readers(&first) && has_readers(&second)) {
        std::thread::sleep(POLL_PERIOD);
    }
    for number in 0..=COMPLETE_STEPS {
        let stamp = FIRST_STEP_NANOS.saturating_add(STEP_PERIOD_NANOS.saturating_mul(number));
        let stamp = i64::try_from(stamp).expect("a step time fits the stamp");
        let (for_first, for_second) =
            (borrow_frame(&first, tag, number), borrow_frame(&second, tag, number));
        safe::loan_send(for_first, stamp).expect("send the first output");
        if number == COMPLETE_STEPS {
            say("between");
            let _unsent = for_second;
            sleep_for_ever()
        }
        safe::loan_send(for_second, stamp).expect("send the second output");
        std::thread::sleep(Duration::from_nanos(STEP_PERIOD_NANOS));
    }
    sleep_for_ever()
}

fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let tag = arguments.get(3).and_then(|text| text.bytes().next());
    let [mode, node, topic, ..] = arguments.as_slice() else {
        eprintln!("usage: hypha_node_kill_peer <flood|read|hold|loan|step> <node> <topic> [tag]");
        return std::process::ExitCode::from(2);
    };
    match (mode.as_str(), tag) {
        ("flood", Some(tag)) => flood(&BusNode::new(node).expect("node"), topic, tag),
        ("read", _) => read(&BusNode::new(node).expect("node"), topic),
        ("hold", _) => hold(&BusNode::new(node).expect("node"), topic),
        ("loan", Some(tag)) => {
            loan(&safe::node_create(node, None, None).expect("node"), topic, tag)
        }
        ("step", Some(tag)) => {
            step(&safe::node_create(node, None, None).expect("node"), topic, tag)
        }
        _ => {
            eprintln!("unknown mode '{mode}', or a publishing mode without a tag");
            std::process::ExitCode::from(2)
        }
    }
}
