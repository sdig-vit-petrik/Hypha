//! The caller-owned `WaitSet` serves event, periodic, and silence work on the calling thread.
#![allow(clippy::expect_used)]

mod support;
use support::sealed_send;

use core::time::Duration;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hypha_core::port::{
    QosPolicy, QosRequest, TopicSpec,
    dispatch::{AttachmentId, StopReason},
};
use hypha_iceoryx2::waitset::{Dispatcher, EventOutcome, Progression, Trigger};
use hypha_iceoryx2::{Iox2Subscriber, Iox2Transport};
use hypha_testkit::unique_name;

const BUDGET: u32 = 64;

/// The policy of a subscriber that creates its own service.
fn declared_qos() -> QosRequest {
    QosRequest::Declared(QosPolicy::new("waitset", 8, 16, 8))
}

/// Counts what one run delivered, then stops the run after enough work.
struct CountingDispatcher {
    endpoints: Vec<(AttachmentId, Iox2Subscriber)>,
    data: Vec<Vec<u8>>,
    intervals: u64,
    stop_after_data: usize,
    stop_after_intervals: u64,
}

impl CountingDispatcher {
    fn new(stop_after_data: usize, stop_after_intervals: u64) -> Self {
        Self {
            endpoints: Vec::new(),
            data: Vec::new(),
            intervals: 0,
            stop_after_data,
            stop_after_intervals,
        }
    }

    fn own(&mut self, attachment: AttachmentId, endpoint: Iox2Subscriber) {
        self.endpoints.push((attachment, endpoint));
    }

    fn enough(&self) -> Progression {
        let data_done = self.stop_after_data > 0 && self.data.len() >= self.stop_after_data;
        let intervals_done =
            self.stop_after_intervals > 0 && self.intervals >= self.stop_after_intervals;
        if data_done || intervals_done {
            return Progression::Stop;
        }
        Progression::Continue
    }
}

impl Dispatcher for CountingDispatcher {
    fn on_event(&mut self, attachment: AttachmentId) -> EventOutcome {
        let Some(entry) = self.endpoints.iter_mut().find(|(id, _)| *id == attachment) else {
            return EventOutcome::Drained;
        };
        let received = match entry.1.try_receive() {
            Ok(Some(sample)) => sample,
            Ok(None) => return EventOutcome::Drained,
            Err(cause) => return EventOutcome::Fail(cause),
        };
        self.data.push(received.payload().to_vec());
        match self.enough() {
            Progression::Stop => EventOutcome::Stop,
            Progression::Fail(cause) => EventOutcome::Fail(cause),
            Progression::Continue => EventOutcome::MoreWork,
        }
    }

    fn on_interval(&mut self, _attachment: AttachmentId, _missed: u64) -> Progression {
        self.intervals = self.intervals.saturating_add(1);
        self.enough()
    }
}

#[test]
fn an_event_driven_subscription_delivers_queued_data_on_the_calling_thread() {
    let topic = format!("/{}", unique_name("waitset_event"));
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data");

    let receiver = Iox2Transport::with_name(&unique_name("waitset_event_rx")).expect("rx");
    let sender = Iox2Transport::with_name(&unique_name("waitset_event_tx")).expect("tx");
    let publisher = sender.publish(&spec).expect("publish");
    let subscriber = receiver.subscribe(&spec).expect("subscribe");

    let mut waitset = receiver.waitset(&unique_name("waitset_event_ctl")).expect("waitset");
    let subscriber = subscriber;
    let listener = subscriber.open_listener().expect("open a listener");
    let attachment = waitset.attach(listener, Trigger::EventDriven).expect("attach");

    let mut loan = publisher.loan(4).expect("loan");
    loan.as_mut_bytes().copy_from_slice(b"scan");
    loan.send_frame(sealed_send::metadata(), 0).expect("send");

    let mut dispatcher = CountingDispatcher::new(1, 0);
    dispatcher.own(attachment, subscriber);
    let report = waitset.run(BUDGET, &mut dispatcher);

    assert_eq!(report.reason, StopReason::Requested);
    assert_eq!(dispatcher.data, vec![b"scan".to_vec()]);
}

#[test]
fn a_periodic_subscription_ticks_without_any_publisher() {
    let topic = format!("/{}", unique_name("waitset_periodic"));
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data").with_qos(declared_qos());

    let receiver = Iox2Transport::with_name(&unique_name("waitset_periodic_rx")).expect("rx");
    let subscriber = receiver.subscribe(&spec).expect("subscribe");

    let mut waitset = receiver.waitset(&unique_name("waitset_periodic_ctl")).expect("waitset");
    let subscriber = subscriber;
    let listener = subscriber.open_listener().expect("open a listener");
    let attachment =
        waitset.attach(listener, Trigger::Periodic(Duration::from_millis(10))).expect("attach");

    let mut dispatcher = CountingDispatcher::new(0, 3);
    dispatcher.own(attachment, subscriber);
    let report = waitset.run(BUDGET, &mut dispatcher);

    assert_eq!(report.reason, StopReason::Requested);
    assert!(dispatcher.intervals >= 3, "the interval must tick without a publisher");
}

#[test]
fn a_stop_handle_ends_a_run_from_another_thread() {
    let topic = format!("/{}", unique_name("waitset_stop"));
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data").with_qos(declared_qos());

    let receiver = Iox2Transport::with_name(&unique_name("waitset_stop_rx")).expect("rx");
    let subscriber = receiver.subscribe(&spec).expect("subscribe");
    let mut waitset = receiver.waitset(&unique_name("waitset_stop_ctl")).expect("waitset");
    let subscriber = subscriber;
    let listener = subscriber.open_listener().expect("open a listener");
    let attachment =
        waitset.attach(listener, Trigger::Periodic(Duration::from_millis(10))).expect("attach");

    let handle = waitset.stop_handle();
    let ticks = Arc::new(AtomicUsize::new(0));
    let stopper_ticks = Arc::clone(&ticks);
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        stopper_ticks.store(1, Ordering::SeqCst);
        handle.request_stop().expect("stop");
    });

    let mut dispatcher = CountingDispatcher::new(0, 0);
    dispatcher.own(attachment, subscriber);
    let report = waitset.run(BUDGET, &mut dispatcher);
    stopper.join().expect("join");

    assert_eq!(report.reason, StopReason::Requested);
    assert_eq!(ticks.load(Ordering::SeqCst), 1, "the stop came from the other thread");
}

#[test]
fn two_waitsets_run_on_two_application_threads() {
    let topic_a = format!("/{}", unique_name("waitset_pair_a"));
    let topic_b = format!("/{}", unique_name("waitset_pair_b"));

    let transport_a = Iox2Transport::with_name(&unique_name("waitset_pair_rx_a")).expect("rx a");
    let transport_b = Iox2Transport::with_name(&unique_name("waitset_pair_rx_b")).expect("rx b");
    let subscriber_a = transport_a
        .subscribe(&TopicSpec::new(&topic_a).with_type("hypha_test/Data").with_qos(declared_qos()))
        .expect("sub a");
    let subscriber_b = transport_b
        .subscribe(&TopicSpec::new(&topic_b).with_type("hypha_test/Data").with_qos(declared_qos()))
        .expect("sub b");

    let mut waitset_a = transport_a.waitset(&unique_name("waitset_pair_ctl_a")).expect("ws a");
    let mut waitset_b = transport_b.waitset(&unique_name("waitset_pair_ctl_b")).expect("ws b");
    let subscriber_a = subscriber_a;
    let subscriber_b = subscriber_b;
    let listener_a = subscriber_a.open_listener().expect("open a listener");
    let listener_b = subscriber_b.open_listener().expect("open a listener");
    let attachment_a =
        waitset_a.attach(listener_a, Trigger::Periodic(Duration::from_millis(10))).expect("a");
    let attachment_b =
        waitset_b.attach(listener_b, Trigger::Periodic(Duration::from_millis(10))).expect("b");

    let worker = std::thread::spawn(move || {
        let mut dispatcher = CountingDispatcher::new(0, 2);
        dispatcher.own(attachment_b, subscriber_b);
        let report = waitset_b.run(BUDGET, &mut dispatcher);
        (report.reason, dispatcher.intervals)
    });

    let mut dispatcher = CountingDispatcher::new(0, 2);
    dispatcher.own(attachment_a, subscriber_a);
    let report_a = waitset_a.run(BUDGET, &mut dispatcher);
    let (reason_b, intervals_b) = worker.join().expect("join");

    assert_eq!(report_a.reason, StopReason::Requested);
    assert_eq!(reason_b, StopReason::Requested);
    assert!(dispatcher.intervals >= 2);
    assert!(intervals_b >= 2);
}

/// Count the threads of this process through the operating system.
///
/// Process-global: needs one process per test, as nextest gives.
fn thread_count() -> usize {
    let pid = std::process::id();
    let output =
        std::process::Command::new("ps").args(["-M", &pid.to_string()]).output().expect("ps -M");
    String::from_utf8_lossy(&output.stdout).lines().count().saturating_sub(1)
}

#[test]
fn the_caller_owned_path_adds_no_thread() {
    let topic = format!("/{}", unique_name("waitset_nothread"));
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data").with_qos(declared_qos());

    let before = thread_count();
    let receiver = Iox2Transport::with_name(&unique_name("waitset_nothread_rx")).expect("rx");
    let subscriber = receiver.subscribe(&spec).expect("subscribe");
    let mut waitset = receiver.waitset(&unique_name("waitset_nothread_ctl")).expect("waitset");
    let subscriber = subscriber;
    let listener = subscriber.open_listener().expect("open a listener");
    waitset.attach(listener, Trigger::EventDriven).expect("attach");
    let after = thread_count();

    println!("M1 threads: before={before} after={after}");
    assert_eq!(after, before, "the caller-owned path must create no thread");
}

/// A bounded run takes one dispatch turn even when its limit passed before the first one.
#[test]
fn a_bounded_run_takes_one_turn_even_when_its_limit_passed_during_setup() {
    let topic = format!("/{}", unique_name("waitset_one_turn"));
    let spec = TopicSpec::new(&topic).with_type("hypha_test/Data");

    let receiver = Iox2Transport::with_name(&unique_name("waitset_one_turn_rx")).expect("rx");
    let sender = Iox2Transport::with_name(&unique_name("waitset_one_turn_tx")).expect("tx");
    let publisher = sender.publish(&spec).expect("publish");
    let subscriber = receiver.subscribe(&spec).expect("subscribe");

    let mut waitset = receiver.waitset(&unique_name("waitset_one_turn_ctl")).expect("waitset");
    let listener = subscriber.open_listener().expect("open a listener");
    let attachment = waitset.attach(listener, Trigger::EventDriven).expect("attach");

    let mut loan = publisher.loan(4).expect("loan");
    loan.as_mut_bytes().copy_from_slice(b"scan");
    loan.send_frame(sealed_send::metadata(), 0).expect("send");

    let mut dispatcher = CountingDispatcher::new(0, 0);
    dispatcher.own(attachment, subscriber);
    // One nanosecond has passed before the run first looks at its deadline.
    let report = waitset.run_for(BUDGET, Duration::from_nanos(1), &mut dispatcher);

    assert_eq!(report.reason, StopReason::Deadline);
    assert_eq!(dispatcher.data, vec![b"scan".to_vec()], "the queued sample was not served");
}
