//! One publish is one sample in shared memory, whatever the number of subscribers (FSR-02).
//!
//! The publisher writes a sample once, into one slot of its segment, and every
//! subscriber reads it there: a publish hands each subscriber one reference to that
//! slot and copies no payload. The tests count and compare addresses; none of them
//! measures time, so none depends on the speed of the host. That the hand-over itself
//! is one queue entry per subscriber connection is the analysis half of FSR-02.
//!
//! Every subscriber maps the publisher's segment at an address of its own, so the
//! addresses differ and the offsets into the segment are what is equal:
//! - on every system, each publish takes a slot of its own, reaches each subscriber
//!   once, and a subscriber's view of two samples is the publisher's view of them
//!   shifted by one constant. That rules out a copy on the heap for each receive; it
//!   does not rule out a copy into a pool laid out like the publisher's segment.
//! - on Linux, `/proc/self/maps` names the file and the offset behind each address,
//!   and they are the publisher's for every subscriber. This is the proof of one
//!   shared sample, and it runs on Linux only.

#![allow(clippy::expect_used, reason = "a failed setup or receive is a test failure")]

use std::collections::BTreeSet;

use hypha_core::port::{PayloadCapacity, QosPolicy, QosRequest, TopicSpec};
use hypha_iceoryx2::{
    Iox2Publisher, Iox2Sample, Iox2Subscriber, Iox2Transport, MAX_BORROWED_SAMPLES,
};
use hypha_testkit::unique_name;

mod support;

/// The subscriber counts FSR-02 is checked for.
const SUBSCRIBER_COUNTS: [usize; 4] = [1, 2, 4, 8];
/// Large enough that a copy for each subscriber would be a real cost.
const PAYLOAD_BYTES: usize = 64 * 1024;
/// Samples a subscriber holds at once, so each publish needs a slot of its own.
const HELD_SAMPLES: usize = MAX_BORROWED_SAMPLES;

/// One published sample as its publisher saw it: where it wrote the payload, and the
/// byte it filled the payload with.
struct Published {
    address: usize,
    fill: u8,
}

/// A publisher on a node of its own and `count` subscribers, every second one on
/// that node and the others on `listening`.
struct Topic {
    publisher: Iox2Publisher,
    subscribers: Vec<Iox2Subscriber>,
    /// The publisher's node, kept for as long as its ports.
    _publishing: Iox2Transport,
}

impl Topic {
    fn with_subscribers(count: usize, listening: &Iox2Transport) -> Self {
        let publishing = Iox2Transport::with_name(&unique_name("fsr02_pub")).expect("transport");
        let policy = QosPolicy::new("fsr02", 1, count, HELD_SAMPLES);
        let spec = TopicSpec::new(&format!("/{}", unique_name("fsr02_topic")))
            .with_type("hypha_test/Fsr02")
            .keeping(HELD_SAMPLES)
            .with_payload_capacity(PayloadCapacity::Bounded(PAYLOAD_BYTES))
            .with_qos(QosRequest::Declared(policy));
        let publisher = publishing.publish(&spec).expect("publisher");
        let subscribers = (0..count)
            .map(|index| {
                let transport = if index % 2 == 0 { &publishing } else { listening };
                transport.subscribe(&spec).expect("subscriber")
            })
            .collect();
        Self { publisher, subscribers, _publishing: publishing }
    }

    /// Publishes [`HELD_SAMPLES`] samples and returns what the publisher saw of each.
    fn publish_samples(&self) -> Vec<Published> {
        (0..HELD_SAMPLES)
            .map(|index| {
                let fill = u8::try_from(index).expect("a few samples").wrapping_add(0xa0);
                let mut loan = self.publisher.loan(PAYLOAD_BYTES).expect("loan");
                loan.as_mut_bytes().fill(fill);
                let address = loan.as_bytes().as_ptr().addr();
                loan.send_frame(support::sealed_send::metadata(), 0).expect("send");
                Published { address, fill }
            })
            .collect()
    }
}

/// Takes one sample of `subscriber` for each publish and keeps them all, so a slot
/// is not used again while the test compares addresses.
fn receive_each(subscriber: &Iox2Subscriber, published: &[Published]) -> Vec<Iox2Sample> {
    published
        .iter()
        .map(|sent| {
            let sample = subscriber.try_receive().expect("receive").expect("a sample per publish");
            assert_eq!(sample.payload().len(), PAYLOAD_BYTES);
            assert!(sample.payload().iter().all(|byte| *byte == sent.fill), "the payload as sent");
            sample
        })
        .collect()
}

/// The distance from the publisher's address of a sample to a subscriber's address of it.
fn shift(sample: &Iox2Sample, sent: &Published) -> usize {
    sample.payload().as_ptr().addr().wrapping_sub(sent.address)
}

#[test]
fn every_subscriber_reads_a_sample_in_the_slot_its_publisher_wrote() {
    let listening = Iox2Transport::with_name(&unique_name("fsr02_sub")).expect("transport");
    for count in SUBSCRIBER_COUNTS {
        let topic = Topic::with_subscribers(count, &listening);
        let published = topic.publish_samples();
        let slots: BTreeSet<usize> = published.iter().map(|sent| sent.address).collect();
        assert_eq!(slots.len(), published.len(), "each publish took a slot of its own");

        for subscriber in &topic.subscribers {
            let samples = receive_each(subscriber, &published);
            let shifts: BTreeSet<usize> =
                samples.iter().zip(&published).map(|(sample, sent)| shift(sample, sent)).collect();
            assert_eq!(
                shifts.len(),
                1,
                "{count} subscribers: a subscriber's samples lie as the publisher's slots do"
            );
            drop(samples);
            let again = subscriber.try_receive().expect("receive");
            assert!(again.is_none(), "{count} subscribers: a publish reaches a subscriber once");
        }
    }
}

/// The file behind `address` and the offset of `address` in it, from the text of
/// `/proc/self/maps`: `start-end perms offset device inode path`, numbers in hex.
#[cfg(target_os = "linux")]
fn file_location(maps: &str, address: usize) -> (String, u64) {
    let hex = |text: &str| u64::from_str_radix(text, 16).expect("a hexadecimal number");
    let address = u64::try_from(address).expect("an address fits 64 bits");
    maps.lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let (start, end) = fields.next()?.split_once('-')?;
            let (_permissions, offset, device, inode) =
                (fields.next()?, fields.next()?, fields.next()?, fields.next()?);
            let (start, end) = (hex(start), hex(end));
            (start..end).contains(&address).then(|| {
                let in_mapping = address.checked_sub(start).expect("the address is in the range");
                let in_file = hex(offset).checked_add(in_mapping).expect("a file offset");
                (format!("{device}:{inode}"), in_file)
            })
        })
        .expect("every address of the process is in a mapping")
}

#[cfg(target_os = "linux")]
#[test]
fn every_subscriber_reads_the_publishers_file_at_the_publishers_offset() {
    let listening = Iox2Transport::with_name(&unique_name("fsr02_sub")).expect("transport");
    for count in SUBSCRIBER_COUNTS {
        let topic = Topic::with_subscribers(count, &listening);
        let published = topic.publish_samples();
        let held: Vec<Vec<Iox2Sample>> = topic
            .subscribers
            .iter()
            .map(|subscriber| receive_each(subscriber, &published))
            .collect();
        // A copy on the heap is what this test must tell from the shared sample.
        let copy =
            held.first().and_then(|samples| samples.first()).expect("a sample").payload().to_vec();

        let maps = std::fs::read_to_string("/proc/self/maps").expect("the memory map");
        for (index, sent) in published.iter().enumerate() {
            let written = file_location(&maps, sent.address);
            assert!(!written.0.ends_with(":0"), "the publisher's slot is in a file: {written:?}");
            assert_ne!(file_location(&maps, copy.as_ptr().addr()), written, "a copy is elsewhere");
            for samples in &held {
                let sample = samples.get(index).expect("one sample for each publish");
                let read = file_location(&maps, sample.payload().as_ptr().addr());
                assert_eq!(read, written, "{count} subscribers: one sample, read where written");
            }
        }
    }
}
