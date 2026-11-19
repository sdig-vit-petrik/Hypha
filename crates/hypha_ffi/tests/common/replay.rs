//! Replay fixtures: a simulated clock, a recording cycle step, and an input rig.
//! The clock is process-wide, so a case that moves it holds [`SERIAL`].

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::ffi::c_void;
use std::fmt::Write as _;
use std::ptr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

use crate::common::{c_text, create_node, create_sim_node_with_file, unique};

pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

/// The period of every cycle in these tests, 50 Hz.
pub(crate) const PERIOD_NANOS: i64 = 20_000_000;
pub(crate) const MILLISECOND: i64 = 1_000_000;
pub(crate) const SECOND: i64 = 1_000_000_000;

pub(crate) struct SimulatedClock {
    _serial: MutexGuard<'static, ()>,
}

impl SimulatedClock {
    pub(crate) fn start(nanos: i64) -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(hypha_clock_enable_sim_time(nanos), HyphaStatus::Ok as i32);
        Self { _serial: serial }
    }
}

pub(crate) fn set_clock(nanos: i64) {
    assert_eq!(hypha_clock_set_sim_time(nanos), HyphaStatus::Ok as i32);
}

impl Drop for SimulatedClock {
    fn drop(&mut self) {
        assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) scheduled: i64,
    pub(crate) missed: u64,
    pub(crate) sample_count: usize,
    pub(crate) verdict: u32,
    /// The first byte of the newest selected sample, `None` when there is none.
    pub(crate) newest: Option<u8>,
}

/// Where the step records its work, and how long it sleeps.
pub(crate) struct Recorder {
    pub(crate) steps: Mutex<Vec<Step>>,
    pub(crate) throttle: Duration,
}

impl Recorder {
    pub(crate) fn new(throttle: Duration) -> Self {
        Self { steps: Mutex::new(Vec::new()), throttle }
    }

    pub(crate) fn steps(&self) -> Vec<Step> {
        self.steps.lock().expect("steps").clone()
    }
}

pub(crate) extern "C" fn record_step(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let recorder = unsafe { &*context.cast::<Recorder>() };
    if !recorder.throttle.is_zero() {
        std::thread::sleep(recorder.throttle);
    }
    let time = unsafe { *time };
    // A cycle without an input has no binding 0: its report stays empty.
    let mut binding = HyphaCycleBindingView::default();
    let _no_binding = unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) };
    let report = binding.health.capture;
    let mut newest = None;
    if report.sample_count > 0 {
        let mut view = HyphaCycleSampleView {
            payload: ptr::null(),
            payload_len: 0,
            transport_sent_nanos: 0,
            sequence: 0,
            publisher_id: 0,
            publisher_sequence: 0,
        };
        let last = report.sample_count.saturating_sub(1);
        assert_eq!(
            unsafe { hypha_cycle_image_sample(image, 0, last, &raw mut view) },
            HyphaStatus::Ok
        );
        newest = Some(unsafe { *view.payload });
    }
    let step = Step {
        scheduled: time.scheduled_nanos,
        missed: time.missed_periods,
        sample_count: report.sample_count,
        verdict: report.verdict,
        newest,
    };
    recorder.steps.lock().expect("steps").push(step);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

pub(crate) fn run_for(waitset: *mut HyphaWaitSet, nanos: u64) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, nanos, &raw mut report) }, HyphaStatus::Ok);
    assert_eq!(report.status, HyphaStatus::Ok as i32, "the run failed");
}

/// Run `waitset` for `nanos` and return the status of the run.
#[allow(dead_code, reason = "not every test binary checks a failed run")]
pub(crate) fn run_status(waitset: *mut HyphaWaitSet, nanos: u64) -> i32 {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    assert_eq!(unsafe { hypha_waitset_run(waitset, nanos, &raw mut report) }, HyphaStatus::Ok);
    report.status
}

pub(crate) unsafe fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    waitset
}

/// A publisher and a subscriber on one topic, with the given queue depth.
pub(crate) unsafe fn create_pubsub(
    node: *mut HyphaNode,
    topic_text: &str,
    depth: usize,
) -> (*mut HyphaPublisher, *mut HyphaSubscriber) {
    let topic = c_text(topic_text);
    let data_type = c_text("std_msgs/Counter");
    let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
    let definition = c_text("");
    let pub_qos = HyphaPublisherOptions {
        latch: 0,
        loaned_samples: 0,
        qos: crate::common::qos_for_topic(topic_text),
    };
    let mut publisher = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publish_with_qos(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                data_type.as_ptr(),
                data_type.as_bytes().len(),
                md5.as_ptr(),
                md5.as_bytes().len(),
                definition.as_ptr(),
                0,
                &raw const pub_qos,
                &raw mut publisher,
            )
        },
        HyphaStatus::Ok
    );
    let sub_qos = HyphaSubscriberOptions {
        depth,
        no_publisher_warning_after_nanos: 0,
        qos: hypha_ffi::HyphaQos::default(),
    };
    let mut subscriber = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_subscribe_typed(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                c"std_msgs/Data".as_ptr(),
                "std_msgs/Data".len(),
                md5.as_ptr(),
                md5.as_bytes().len(),
                std::ptr::null(),
                0,
                &raw const sub_qos,
                &raw mut subscriber,
            )
        },
        HyphaStatus::Ok
    );
    (publisher, subscriber)
}

/// Send a one-byte sample stamped with framework time `stamp_nanos`.
pub(crate) unsafe fn send_stamped(publisher: *mut HyphaPublisher, value: u8, stamp_nanos: i64) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(publisher, 1, &raw mut loan, &raw mut bytes) },
        HyphaStatus::Ok
    );
    unsafe { bytes.write(value) };
    assert!(stamp_nanos >= 0, "a stamp is not negative");
    assert_eq!(unsafe { hypha_loan_send(loan, stamp_nanos) }, HyphaStatus::Ok);
}

/// One node with a cycle that reads one managed input, committed on the current clock.
pub(crate) struct Rig {
    pub(crate) node: *mut HyphaNode,
    pub(crate) waitset: *mut HyphaWaitSet,
    pub(crate) publisher: *mut HyphaPublisher,
    pub(crate) source: *mut HyphaInputSource,
    pub(crate) cycle: *mut HyphaCycle,
    pub(crate) recorder: Box<Recorder>,
}

impl Rig {
    /// `max_age` is the silence limit in nanoseconds (0 for none), and `history` the
    /// (samples, bytes) limit of the input history.
    pub(crate) fn new(
        label: &str,
        mode: HyphaCycleInputMode,
        max_age: u64,
        history: (usize, usize),
    ) -> Self {
        Self::with_step_time(label, mode, max_age, history, Duration::ZERO)
    }

    /// [`Rig::new`] whose step takes `step_time`.
    pub(crate) fn with_step_time(
        label: &str,
        mode: HyphaCycleInputMode,
        max_age: u64,
        _history: (usize, usize),
        step_time: Duration,
    ) -> Self {
        // The silence limit is a key of the input's table.
        let table = format!("[ports.inputs.replay_in]\nsilence_ms = {}\n", max_age / 1_000_000);
        let node = if max_age == 0 {
            create_node(&unique(label))
        } else {
            crate::common::supervision::node_with_parameter_text(label, &table)
        };
        let waitset = unsafe { create_waitset(node) };
        let topic = format!("/{}", unique("replay_in"));
        let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 32) };
        let mut source = ptr::null_mut();
        assert_eq!(
            unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
            HyphaStatus::Ok
        );
        let recorder = Box::new(Recorder::new(step_time));
        let name = c_text(&unique("replay_cycle"));
        let mut cycle = ptr::null_mut();
        assert_eq!(
            unsafe {
                hypha_cycle_create(
                    waitset,
                    name.as_ptr(),
                    name.as_bytes().len(),
                    u64::try_from(PERIOD_NANOS).expect("period"),
                    Some(record_step),
                    (&raw const *recorder).cast_mut().cast(),
                    None,
                    &raw mut cycle,
                )
            },
            HyphaStatus::Ok
        );
        let bind = HyphaCycleBindingOptions {
            mode: mode as u32,
            max_samples_per_iteration: 64,
            optional: 0,
        };
        let mut index = 0_u32;
        assert_eq!(
            unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
            HyphaStatus::Ok
        );
        if max_age != 0 {
            let bound = unsafe {
                crate::common::supervision::bind(
                    cycle,
                    node,
                    index,
                    "replay_in",
                    (true, false, true),
                )
            };
            assert_eq!((bound.status, bound.attached), (HyphaStatus::Ok, 1), "{}", bound.detail);
        }
        assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
        Self { node, waitset, publisher, source, cycle, recorder }
    }

    pub(crate) fn steps(&self) -> Vec<Step> {
        self.recorder.steps()
    }

    /// Run the `WaitSet` for a moment; every deadline the clock has passed runs in that turn.
    pub(crate) fn settle(&self) {
        run_for(self.waitset, 5_000_000);
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        assert_eq!(unsafe { hypha_cycle_destroy(self.cycle) }, HyphaStatus::Ok);
        assert_eq!(unsafe { hypha_input_source_destroy(self.source) }, HyphaStatus::Ok);
        unsafe {
            hypha_publisher_destroy(self.publisher);
            hypha_node_destroy(self.node);
        }
    }
}

/// The MD5 of `rosgraph_msgs/Clock`, the identity of `/clock` frames.
pub(crate) const CLOCK_MD5: &str = "a9c97c1d230cfc112e270351a944ee47";

/// A node that runs on simulated time. The parameter file is written, read, and removed.
pub(crate) fn create_sim_node(label: &str) -> *mut HyphaNode {
    create_sim_node_with_triggers(label, &[])
}

/// A node on simulated time with one trigger table per limit in `silences_ms`
/// (`crate::common::supervision::trigger_port`).
pub(crate) fn create_sim_node_with_triggers(label: &str, silences_ms: &[i64]) -> *mut HyphaNode {
    let path = std::env::temp_dir().join(format!("{label}.toml"));
    let mut text = String::new();
    for ms in silences_ms {
        let port = crate::common::supervision::trigger_port(*ms);
        writeln!(text, "[ports.inputs.{port}]\nsilence_ms = {ms}").expect("write to a String");
    }
    std::fs::write(&path, text).expect("write the parameter file");
    let node = create_sim_node_with_file(label, Some(&path));
    std::fs::remove_file(&path).unwrap_or_default();
    node
}

/// A publisher of `topic` that declares the test policy, as every test subscriber
/// does, so either endpoint may open the topic first.
pub(crate) unsafe fn open_publisher(
    node: *mut HyphaNode,
    topic: &str,
    data_type: &str,
    md5: &str,
    definition: &str,
) -> *mut HyphaPublisher {
    let policy = crate::common::qos_for_topic(topic);
    unsafe { open_publisher_with(node, topic, data_type, md5, definition, policy) }
}

/// A publisher of `topic` that declares the test policy; it must be the first opener.
pub(crate) unsafe fn open_declared_publisher(
    node: *mut HyphaNode,
    topic: &str,
    data_type: &str,
    md5: &str,
) -> *mut HyphaPublisher {
    unsafe { open_publisher_with(node, topic, data_type, md5, "", crate::common::test_qos()) }
}

unsafe fn open_publisher_with(
    node: *mut HyphaNode,
    topic: &str,
    data_type: &str,
    md5: &str,
    definition: &str,
    qos: hypha_ffi::HyphaQos,
) -> *mut HyphaPublisher {
    let pub_qos = HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos };
    let topic = c_text(topic);
    let data_type = c_text(data_type);
    let md5 = c_text(md5);
    let definition = c_text(definition);
    let mut publisher = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publish_with_qos(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                data_type.as_ptr(),
                data_type.as_bytes().len(),
                md5.as_ptr(),
                md5.as_bytes().len(),
                definition.as_ptr(),
                definition.as_bytes().len(),
                &raw const pub_qos,
                &raw mut publisher,
            )
        },
        HyphaStatus::Ok
    );
    publisher
}

/// A subscriber on `topic` that validates frames against the identity `md5`.
pub(crate) unsafe fn open_subscriber(
    node: *mut HyphaNode,
    topic: &str,
    md5: &str,
    depth: usize,
) -> *mut HyphaSubscriber {
    // A subscriber may be the first opener of its topic, so it declares the policy
    // the test publishers declare; a framework topic keeps its own limits.
    let policy = crate::common::qos_for_topic(topic);
    let topic = c_text(topic);
    let md5 = c_text(md5);
    let sub_qos =
        HyphaSubscriberOptions { depth, no_publisher_warning_after_nanos: 0, qos: policy };
    let mut subscriber = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_subscribe_typed(
                node,
                topic.as_ptr(),
                topic.as_bytes().len(),
                c"std_msgs/Data".as_ptr(),
                "std_msgs/Data".len(),
                md5.as_ptr(),
                md5.as_bytes().len(),
                std::ptr::null(),
                0,
                &raw const sub_qos,
                &raw mut subscriber,
            )
        },
        HyphaStatus::Ok
    );
    subscriber
}

pub(crate) unsafe fn clock_publisher(node: *mut HyphaNode) -> *mut HyphaPublisher {
    unsafe { open_publisher(node, "/clock", "rosgraph_msgs/Clock", CLOCK_MD5, "") }
}

/// Publish one `/clock` sample that carries `nanos` of simulated time.
pub(crate) unsafe fn publish_clock(publisher: *mut HyphaPublisher, nanos: u64) {
    let secs = u32::try_from(nanos / 1_000_000_000).expect("the clock fits ROS time");
    let subsec = u32::try_from(nanos % 1_000_000_000).expect("the clock fits ROS time");
    let mut payload = [0_u8; 8];
    payload[..4].copy_from_slice(&secs.to_le_bytes());
    payload[4..].copy_from_slice(&subsec.to_le_bytes());
    unsafe { send_payload(publisher, &payload) };
}

pub(crate) unsafe fn send_payload(publisher: *mut HyphaPublisher, payload: &[u8]) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publisher_loan_frame(publisher, payload.len(), &raw mut loan, &raw mut bytes)
        },
        HyphaStatus::Ok
    );
    unsafe { ptr::copy_nonoverlapping(payload.as_ptr(), bytes, payload.len()) };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
}

/// Take the next sample of `subscriber` as its payload, or `None` when none is queued.
pub(crate) unsafe fn receive_payload(subscriber: *mut HyphaSubscriber) -> Option<Vec<u8>> {
    let received = unsafe { crate::common::receive::receive(subscriber) };
    assert!(received.is_ok(), "the receive is refused: {:?}", received.as_ref().err());
    received.ok().flatten().map(|frame| frame.payload)
}

/// Take the next sample of `subscriber`: its payload and its header send times.
pub(crate) unsafe fn receive_frame(
    subscriber: *mut HyphaSubscriber,
) -> Option<(Vec<u8>, crate::common::receive::SendTimes)> {
    let received = unsafe { crate::common::receive::receive(subscriber) };
    assert!(received.is_ok(), "the receive is refused: {:?}", received.as_ref().err());
    received.ok().flatten().map(|frame| (frame.payload, frame.sent))
}
