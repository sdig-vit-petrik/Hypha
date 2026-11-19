//! Test-only peer process of `tests/peer_kill_isolation.rs`. The test kills it with
//! SIGKILL, so it never ends on its own.
//!
//! ```text
//! hypha_node_kill_peer flood <node> <topic> <tag>   publish for ever, 5000 frames a second
//! hypha_node_kill_peer read  <node> <topic>         read every sample and release it
//! hypha_node_kill_peer hold  <node> <topic>         take two samples and keep them
//! ```
//!
//! It prints `ready` when its endpoint exists. `hold` then prints `holding` when it
//! keeps two samples, the most a subscriber may borrow. A frame of `flood` is nine
//! bytes: the tag, then a counter from zero, little endian.

#![allow(clippy::expect_used, reason = "a failed setup ends the peer, and the test reports it")]

use std::io::Write as _;
use std::time::Duration;

use hypha_node::sdk::{BusNode, ReceivedSample};

const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
const FLOOD_PERIOD: Duration = Duration::from_micros(200);
const POLL_PERIOD: Duration = Duration::from_micros(100);
const HELD_SAMPLES: usize = 2;

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

fn main() -> std::process::ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let tag = arguments.get(3).and_then(|text| text.bytes().next());
    let [mode, node, topic, ..] = arguments.as_slice() else {
        eprintln!("usage: hypha_node_kill_peer <flood|read|hold> <node> <topic> [tag]");
        return std::process::ExitCode::from(2);
    };
    let bus = BusNode::new(node).expect("node");
    match (mode.as_str(), tag) {
        ("flood", Some(tag)) => flood(&bus, topic, tag),
        ("read", _) => read(&bus, topic),
        ("hold", _) => hold(&bus, topic),
        _ => {
            eprintln!("unknown mode '{mode}', or a flood without a tag");
            std::process::ExitCode::from(2)
        }
    }
}
