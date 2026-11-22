//! Plain callbacks of a node that follows `/clock` run as events in time order (#72):
//! each sample at its framework send time, each timer period at its deadline, with
//! framework time at the event's time while the callback runs. Every case asserts
//! computed times and orders only (#64).
#![expect(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    reason = "fixed test times far from overflow; a refused setup is a failed test"
)]

use std::ffi::{CString, c_void};
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::Mutex;

mod common;
use crate::common::replay::{MILLISECOND, SECOND, SimulatedClock, run_for, run_status, set_clock};
use crate::common::{attach, create_sim_node_with_file, unique};
use hypha_ffi::waitset::*;
use hypha_ffi::*;

/// The first clock value of every case.
const BASE: i64 = SECOND;
const MS: i64 = MILLISECOND;
/// One run: a run always takes its first turn at once, and this one serves what it found.
const TURN_NANOS: u64 = 1_000_000;

/// What one callback saw: which registration, the event's stamp, and framework time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Call {
    tag: u8,
    stamp: i64,
    now: i64,
}

/// The context of one subscription or timer callback.
struct Tagged {
    tag: u8,
    calls: *const Mutex<Vec<Call>>,
    /// The publisher a subscription callback relays its sample to, or null.
    relay: *mut HyphaPublisher,
}

impl Tagged {
    fn record(&self, call: Call) {
        // SAFETY: `calls` points into the rig's boxed log, which outlives every run.
        let calls = unsafe { &*self.calls };
        calls.lock().expect("calls").push(call);
    }
}

extern "C" fn on_sample(
    delivery: *const HyphaDelivery,
    context: *mut c_void,
) -> HyphaCallbackResult {
    // SAFETY: the rig passes a boxed `Tagged` that lives until the node is destroyed, and
    // the run passes a delivery valid for this call.
    let (tagged, delivery) = unsafe { (&*context.cast::<Tagged>(), &*delivery) };
    let stamp = i64::try_from(delivery.identity.framework_sent_nanos).expect("stamp");
    tagged.record(Call { tag: tagged.tag, stamp, now: hypha_clock_now_nanos() });
    if !tagged.relay.is_null() {
        // Stamped with framework time on this thread: the event's time.
        send(tagged.relay, -1);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

extern "C" fn on_tick(missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    // SAFETY: the rig passes a boxed `Tagged` that lives until the node is destroyed.
    let tagged = unsafe { &*context.cast::<Tagged>() };
    assert_eq!(missed, 0, "a replayed timer misses no period");
    let now = hypha_clock_now_nanos();
    tagged.record(Call { tag: tagged.tag, stamp: now, now });
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// A node on its own simulated clock at [`BASE`], with its `WaitSet`. Endpoints and
/// contexts live until the case ends; the node goes before the clock.
struct Rig {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    topics: Vec<CString>,
    subscribers: Vec<*mut HyphaSubscriber>,
    publishers: Vec<*mut HyphaPublisher>,
    /// Pinned: the C ABI holds each context's address.
    contexts: Vec<Pin<Box<Tagged>>>,
    calls: Box<Mutex<Vec<Call>>>,
    _clock: SimulatedClock,
}

impl Rig {
    fn new(label: &str) -> Self {
        let clock = SimulatedClock::start(BASE);
        let node = create_sim_node_with_file(&unique(label), None);
        let mut waitset = ptr::null_mut();
        // SAFETY: `node` is a live node and `waitset` a valid out pointer.
        let status = unsafe { hypha_node_default_waitset(node, &raw mut waitset) };
        assert_eq!(status, HyphaStatus::Ok);
        Self {
            node,
            waitset,
            topics: Vec::new(),
            subscribers: Vec::new(),
            publishers: Vec::new(),
            contexts: Vec::new(),
            calls: Box::new(Mutex::new(Vec::new())),
            _clock: clock,
        }
    }

    /// A topic of this case, with a publisher on it.
    fn topic(&mut self, name: &str) -> (usize, *mut HyphaPublisher) {
        let topic = CString::new(format!("/{}", unique(name))).expect("topic");
        let publisher = create_publisher(self.node, &topic);
        self.topics.push(topic);
        self.publishers.push(publisher);
        (self.topics.len() - 1, publisher)
    }

    /// Attach a subscription to topic `index` whose calls carry `tag`.
    fn subscribe(&mut self, index: usize, tag: u8, depth: usize, relay: *mut HyphaPublisher) {
        let topic = self.topics.get(index).expect("topic");
        let subscriber = create_subscriber(self.node, topic, depth);
        self.subscribers.push(subscriber);
        let context = self.context(tag, relay);
        self.attach(&attach::subscription(subscriber, Some(on_sample), context, None));
    }

    /// Attach a plain timer of `period` whose calls carry `tag`.
    fn timer(&mut self, period: i64, tag: u8) {
        let context = self.context(tag, ptr::null_mut());
        let period = u64::try_from(period).expect("period");
        self.attach(&attach::timer(period, Some(on_tick), context, None));
    }

    fn attach(&self, record: &HyphaAttachment) {
        let mut attachment = 0;
        // SAFETY: the record names live endpoints and contexts that outlive the node.
        let status = unsafe { hypha_waitset_attach(self.waitset, record, &raw mut attachment) };
        assert_eq!(status, HyphaStatus::Ok);
    }

    fn context(&mut self, tag: u8, relay: *mut HyphaPublisher) -> *mut c_void {
        let calls = &raw const *self.calls;
        let context = Box::pin(Tagged { tag, calls, relay });
        let pointer = (&raw const *context).cast_mut().cast::<c_void>();
        self.contexts.push(context);
        pointer
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("calls").clone()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // SAFETY: each handle came from this rig and is destroyed once, endpoints first.
        unsafe {
            for &publisher in &self.publishers {
                hypha_publisher_destroy(publisher);
            }
            for &subscriber in &self.subscribers {
                hypha_subscriber_destroy(subscriber);
            }
            hypha_node_destroy(self.node);
        }
    }
}

fn create_subscriber(node: *mut HyphaNode, topic: &CString, depth: usize) -> *mut HyphaSubscriber {
    let options = HyphaSubscriberOptions {
        depth,
        no_publisher_warning_after_nanos: 0,
        qos: crate::common::test_qos(),
    };
    let mut subscriber = ptr::null_mut();
    // SAFETY: every pointer is valid for its stated length, and `subscriber` is an out pointer.
    let status = unsafe {
        hypha_subscribe_typed(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            c"std_msgs/Bytes".as_ptr(),
            "std_msgs/Bytes".len(),
            ptr::null(),
            0,
            ptr::null(),
            0,
            &raw const options,
            &raw mut subscriber,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    subscriber
}

fn create_publisher(node: *mut HyphaNode, topic: &CString) -> *mut HyphaPublisher {
    let options =
        HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
    let mut publisher = ptr::null_mut();
    // SAFETY: every pointer is valid for its stated length, and `publisher` is an out pointer.
    let status = unsafe {
        hypha_publish_with_qos(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            c"std_msgs/Bytes".as_ptr(),
            "std_msgs/Bytes".len(),
            c"d9d213e8c146065ec958333c617eb1a6".as_ptr(),
            32,
            c"".as_ptr(),
            0,
            &raw const options,
            &raw mut publisher,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    publisher
}

/// Send one byte, stamped `framework_sent_nanos`, or framework time when negative.
fn send(publisher: *mut HyphaPublisher, framework_sent_nanos: i64) {
    let mut loan = MaybeUninit::<*mut HyphaLoan>::uninit();
    let mut payload = MaybeUninit::<*mut u8>::uninit();
    // SAFETY: `publisher` is live, and both out pointers are valid for a write.
    let loaned = unsafe {
        hypha_publisher_loan_frame(publisher, 1, loan.as_mut_ptr(), payload.as_mut_ptr())
    };
    assert_eq!(loaned, HyphaStatus::Ok);
    // SAFETY: an accepted loan wrote both out pointers.
    let (loan, payload) = unsafe { (loan.assume_init(), payload.assume_init()) };
    let byte = NonNull::new(payload).expect("an accepted loan of one byte has its payload");
    // SAFETY: the loan owns this one writable byte until its send, which consumes the loan.
    let status = unsafe {
        byte.write(1);
        hypha_loan_send(loan, framework_sent_nanos)
    };
    assert_eq!(status, HyphaStatus::Ok);
}

fn sample(tag: u8, stamp: i64) -> Call {
    Call { tag, stamp, now: stamp }
}

#[test]
fn samples_of_two_topics_run_in_stamp_order_at_their_stamps() {
    let mut rig = Rig::new("two_topics");
    let (a, publish_a) = rig.topic("a");
    let (b, publish_b) = rig.topic("b");
    rig.subscribe(a, b'a', 8, ptr::null_mut());
    rig.subscribe(b, b'b', 8, ptr::null_mut());
    // Sent out of stamp order across the two topics; each topic ascends.
    send(publish_b, BASE + 20 * MS);
    send(publish_b, BASE + 40 * MS);
    send(publish_a, BASE + 10 * MS);
    send(publish_a, BASE + 30 * MS);
    set_clock(BASE + 50 * MS);
    run_for(rig.waitset, TURN_NANOS);
    let expected = [
        sample(b'a', BASE + 10 * MS),
        sample(b'b', BASE + 20 * MS),
        sample(b'a', BASE + 30 * MS),
        sample(b'b', BASE + 40 * MS),
    ];
    assert_eq!(rig.calls(), expected, "time order, with framework time at each stamp");
}

#[test]
fn a_sample_waits_until_the_clock_reaches_its_stamp() {
    let mut rig = Rig::new("waits");
    let (a, publish_a) = rig.topic("a");
    rig.subscribe(a, b'a', 8, ptr::null_mut());
    send(publish_a, BASE + 40 * MS);
    set_clock(BASE + 20 * MS);
    run_for(rig.waitset, TURN_NANOS);
    assert_eq!(rig.calls(), [], "ahead of the clock");
    set_clock(BASE + 40 * MS);
    run_for(rig.waitset, TURN_NANOS);
    assert_eq!(rig.calls(), [sample(b'a', BASE + 40 * MS)]);
}

#[test]
fn timers_and_samples_interleave_in_time_order() {
    let mut rig = Rig::new("interleave");
    let (a, publish_a) = rig.topic("a");
    rig.subscribe(a, b'a', 8, ptr::null_mut());
    rig.timer(10 * MS, b't');
    // A sample at a timer's deadline runs after the timer: the timer sees what came before.
    send(publish_a, BASE + 5 * MS);
    send(publish_a, BASE + 20 * MS);
    send(publish_a, BASE + 25 * MS);
    set_clock(BASE + 30 * MS);
    run_for(rig.waitset, TURN_NANOS);
    let expected = [
        sample(b'a', BASE + 5 * MS),
        sample(b't', BASE + 10 * MS),
        sample(b't', BASE + 20 * MS),
        sample(b'a', BASE + 20 * MS),
        sample(b'a', BASE + 25 * MS),
        sample(b't', BASE + 30 * MS),
    ];
    assert_eq!(rig.calls(), expected);
}

#[test]
fn a_callback_publishes_at_the_time_of_its_event() {
    let mut rig = Rig::new("relay");
    let (a, publish_a) = rig.topic("a");
    let (c, publish_c) = rig.topic("c");
    rig.subscribe(a, b'a', 8, publish_c);
    rig.subscribe(c, b'c', 8, ptr::null_mut());
    send(publish_a, BASE + 10 * MS);
    send(publish_a, BASE + 30 * MS);
    set_clock(BASE + 50 * MS);
    run_for(rig.waitset, TURN_NANOS);
    // Each relayed sample carries its cause's time and runs before the next later event.
    let expected = [
        sample(b'a', BASE + 10 * MS),
        sample(b'c', BASE + 10 * MS),
        sample(b'a', BASE + 30 * MS),
        sample(b'c', BASE + 30 * MS),
    ];
    assert_eq!(rig.calls(), expected);
}

#[test]
fn a_sample_stamped_before_the_last_event_fails_the_run() {
    let mut rig = Rig::new("out_of_order");
    let (a, publish_a) = rig.topic("a");
    rig.subscribe(a, b'a', 8, ptr::null_mut());
    send(publish_a, BASE + 30 * MS);
    set_clock(BASE + 40 * MS);
    run_for(rig.waitset, TURN_NANOS);
    send(publish_a, BASE + 20 * MS);
    assert_eq!(run_status(rig.waitset, TURN_NANOS), HyphaStatus::SequenceGap as i32);
    assert_eq!(rig.calls(), [sample(b'a', BASE + 30 * MS)]);
}

#[test]
fn a_sample_the_queue_lost_fails_the_run() {
    let mut rig = Rig::new("lost");
    let (a, publish_a) = rig.topic("a");
    rig.subscribe(a, b'a', 2, ptr::null_mut());
    // A gap is counted after the subscriber's first frame, so one goes through first.
    send(publish_a, BASE + MS);
    set_clock(BASE + MS);
    run_for(rig.waitset, TURN_NANOS);
    // Five more samples into a queue of two: three are lost, and the replay is not faithful.
    for index in 2..7 {
        send(publish_a, BASE + index * MS);
    }
    set_clock(BASE + 10 * MS);
    assert_eq!(run_status(rig.waitset, TURN_NANOS), HyphaStatus::SequenceGap as i32);
    assert_eq!(rig.calls(), [sample(b'a', BASE + MS)], "nothing after the gap ran");
}

#[test]
fn simulated_time_going_back_ends_the_run() {
    let mut rig = Rig::new("rewind");
    let (a, publish_a) = rig.topic("a");
    rig.subscribe(a, b'a', 8, ptr::null_mut());
    send(publish_a, BASE + 10 * MS);
    set_clock(BASE + 20 * MS);
    run_for(rig.waitset, TURN_NANOS);
    set_clock(BASE + 5 * MS);
    assert_eq!(run_status(rig.waitset, TURN_NANOS), HyphaStatus::ClockRewound as i32);
}

/// A node that follows the bus `/clock`: its clock publisher, a reader of
/// `/clock_progress`, and a topic `a` that a second node, `upstream`, publishes and on
/// which it reports its progress by hand.
struct Bus {
    _serial: std::sync::MutexGuard<'static, ()>,
    node: *mut HyphaNode,
    name: String,
    waitset: *mut HyphaWaitSet,
    clock: *mut HyphaPublisher,
    progress: *mut HyphaSubscriber,
    upstream: *mut HyphaNode,
    upstream_name: String,
    data: *mut HyphaPublisher,
    reports: *mut HyphaPublisher,
    subscriber: *mut HyphaSubscriber,
    _context: Pin<Box<Tagged>>,
    calls: Box<Mutex<Vec<Call>>>,
}

impl Bus {
    fn new(label: &str) -> Self {
        use crate::common::replay::{
            SERIAL, clock_publisher, create_sim_node, open_publisher, open_subscriber,
        };
        use hypha_msgs::hypha_msgs::ReplayProgress;
        let serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let name = unique(label);
        let node = create_sim_node(&name);
        let upstream_name = unique("upstream");
        let upstream = crate::common::create_node(&upstream_name);
        let topic = CString::new(format!("/{}", unique("a"))).expect("topic");
        let calls = Box::new(Mutex::new(Vec::new()));
        let context =
            Box::pin(Tagged { tag: b'a', calls: &raw const *calls, relay: ptr::null_mut() });
        let mut waitset = ptr::null_mut();
        // SAFETY: the nodes are live, each out pointer is valid, and the context outlives
        // the node that runs its callback.
        let (clock, progress, data, reports, subscriber) = unsafe {
            assert_eq!(hypha_node_default_waitset(node, &raw mut waitset), HyphaStatus::Ok);
            let clock = clock_publisher(node);
            let progress = open_subscriber(node, "/clock_progress", ReplayProgress::SCHEMA_HASH, 8);
            let reports = open_publisher(
                upstream,
                "/clock_progress",
                "hypha_msgs/ReplayProgress",
                ReplayProgress::SCHEMA_HASH,
                "",
            );
            (
                clock,
                progress,
                create_publisher(upstream, &topic),
                reports,
                create_subscriber(node, &topic, 8),
            )
        };
        let pointer = (&raw const *context).cast_mut().cast::<c_void>();
        let record = attach::subscription(subscriber, Some(on_sample), pointer, None);
        let mut attachment = 0;
        // SAFETY: the record names a live subscriber and a context that outlives the node.
        let status =
            unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut attachment) };
        assert_eq!(status, HyphaStatus::Ok);
        Self {
            _serial: serial,
            node,
            name,
            waitset,
            clock,
            progress,
            upstream,
            upstream_name,
            data,
            reports,
            subscriber,
            _context: context,
            calls,
        }
    }

    fn clock(&self, nanos: i64) {
        let nanos = u64::try_from(nanos).expect("clock");
        // SAFETY: the clock publisher is live.
        unsafe { crate::common::replay::publish_clock(self.clock, nanos) };
    }

    /// The upstream reports that it ran every event at or before `nanos`.
    fn upstream_reports(&self, nanos: i64) {
        use hypha_core::message::EncodeInto as _;
        let clock_nanos = u64::try_from(nanos).expect("clock");
        let report = hypha_msgs::hypha_msgs::ReplayProgress {
            node: self.upstream_name.clone(),
            clock_nanos,
            clock_sent_nanos: clock_nanos,
        };
        let mut payload = vec![0_u8; report.encoded_len()];
        let written = report.encode_into(&mut payload).expect("a report encodes");
        payload.truncate(written);
        // SAFETY: the report publisher is live.
        unsafe { crate::common::replay::send_payload(self.reports, &payload) };
    }

    /// Run the bus node for a few turns; it reads `/clock` and the reports first.
    fn run(&self) {
        run_for(self.waitset, 50 * MS as u64);
    }

    /// The clock of the node's newest progress report, and none before it is read.
    fn last_report(&self) -> Option<u64> {
        use hypha_core::message::DecodeOwned as _;
        let mut last = None;
        // SAFETY: the progress reader is live; each payload is owned once received.
        while let Some(payload) = unsafe { crate::common::replay::receive_payload(self.progress) } {
            let report =
                hypha_msgs::hypha_msgs::ReplayProgress::decode_from(&payload).expect("report");
            if report.node == self.name {
                last = Some(report.clock_nanos);
            }
        }
        last
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("calls").clone()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        // SAFETY: each handle came from this rig and is destroyed once, endpoints first.
        unsafe {
            hypha_publisher_destroy(self.data);
            hypha_publisher_destroy(self.reports);
            hypha_publisher_destroy(self.clock);
            hypha_subscriber_destroy(self.subscriber);
            hypha_subscriber_destroy(self.progress);
            hypha_node_destroy(self.upstream);
            hypha_node_destroy(self.node);
        }
        assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
    }
}

fn as_report(nanos: i64) -> u64 {
    u64::try_from(nanos).expect("clock")
}

#[test]
fn a_sample_waits_for_its_upstream_and_the_report_stays_at_the_upstream_clock() {
    let bus = Bus::new("held");
    bus.upstream_reports(BASE);
    bus.clock(BASE);
    bus.run();
    assert_eq!(
        bus.last_report(),
        Some(as_report(BASE)),
        "the node takes part from its first clock"
    );

    // The upstream sent a sample at 20 ms, but has reported only to 10 ms.
    send(bus.data, BASE + 20 * MS);
    bus.upstream_reports(BASE + 10 * MS);
    bus.clock(BASE + 50 * MS);
    bus.run();
    bus.run();
    assert_eq!(bus.calls(), [], "held for the upstream");
    assert_eq!(bus.last_report(), Some(as_report(BASE + 10 * MS)), "no further than the upstream");

    bus.upstream_reports(BASE + 50 * MS);
    bus.run();
    assert_eq!(bus.calls(), [sample(b'a', BASE + 20 * MS)]);
    assert_eq!(bus.last_report(), Some(as_report(BASE + 50 * MS)), "the whole chunk");
}

#[test]
fn a_publisher_that_takes_no_part_in_the_replay_is_named_once() {
    use crate::common::replay::{SERIAL, clock_publisher, publish_clock};
    let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = std::env::temp_dir().join(unique("unpaced"));
    std::fs::create_dir_all(&dir).expect("log directory");
    let log = dir.join("node.log");
    let parameters = dir.join("node.toml");
    let table = format!(
        "[log]\nconsole = false\n[log.sink.main]\npath = \"{}\"\nlevel = \"info\"\n",
        log.display()
    );
    std::fs::write(&parameters, table).expect("parameter file");
    let node = create_sim_node_with_file(&unique("follower"), Some(&parameters));
    // A node on host time that reports no progress: it takes no part in the replay.
    let outsider_name = unique("outsider");
    let outsider = crate::common::create_node(&outsider_name);
    let topic = CString::new(format!("/{}", unique("a"))).expect("topic");
    let calls = Box::new(Mutex::new(Vec::new()));
    let context = Box::pin(Tagged { tag: b'a', calls: &raw const *calls, relay: ptr::null_mut() });
    let pointer = (&raw const *context).cast_mut().cast::<c_void>();
    let publisher = create_publisher(outsider, &topic);
    let subscriber = create_subscriber(node, &topic, 8);
    let mut waitset = ptr::null_mut();
    // SAFETY: the nodes and endpoints are live, each out pointer is valid, and the
    // context outlives the node that runs its callback.
    let clock = unsafe {
        assert_eq!(hypha_node_default_waitset(node, &raw mut waitset), HyphaStatus::Ok);
        let record = attach::subscription(subscriber, Some(on_sample), pointer, None);
        let mut attachment = 0;
        assert_eq!(
            hypha_waitset_attach(waitset, &raw const record, &raw mut attachment),
            HyphaStatus::Ok
        );
        clock_publisher(node)
    };
    // SAFETY: the clock publisher is live.
    unsafe { publish_clock(clock, u64::try_from(BASE).expect("clock")) };
    run_for(waitset, 50 * MS as u64);
    send(publisher, BASE + 10 * MS);
    // SAFETY: the clock publisher is live.
    unsafe { publish_clock(clock, u64::try_from(BASE + 20 * MS).expect("clock")) };
    run_for(waitset, 50 * MS as u64);
    run_for(waitset, 50 * MS as u64);
    // Its sample still runs, by its stamp.
    assert_eq!(calls.lock().expect("calls").clone(), [sample(b'a', BASE + 10 * MS)]);
    // SAFETY: each handle is destroyed once, endpoints first.
    unsafe {
        hypha_publisher_destroy(clock);
        hypha_publisher_destroy(publisher);
        hypha_subscriber_destroy(subscriber);
        hypha_node_destroy(outsider);
        hypha_node_destroy(node);
    }
    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
    let text = std::fs::read_to_string(&log).expect("node log");
    let named = text.lines().filter(|line| line.contains("takes no part in the replay")).count();
    assert_eq!(named, 1, "named once: {text}");
    assert!(text.contains(&outsider_name), "the outsider is named: {text}");
    drop(context);
}
