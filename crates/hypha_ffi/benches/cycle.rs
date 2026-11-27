//! Cost of one cycle tick with one input and one output over the real transport, and of
//! one `WaitSet` turn of a replay cycle with idle inputs under simulated time, through
//! the C ABI a C++ node uses.
//! Needs writable `/dev/shm` and `/tmp/iceoryx2`. Run with
//! `cargo bench -p hypha_ffi --bench cycle`. Not a CI gate.
#![expect(clippy::expect_used, reason = "a refused setup or a lost message stops the run")]

use std::ffi::{CString, c_void};
use std::ptr;

use criterion::{Criterion, criterion_group, criterion_main};
use hypha_ffi::waitset::{
    HYPHA_ACTION_CONTINUE, HYPHA_ACTION_FAIL, HyphaCallbackResult, HyphaWaitSetRunReport,
    hypha_waitset_run,
};
use hypha_ffi::{
    HyphaCycle, HyphaCycleBindingOptions, HyphaCycleEmission, HyphaCycleImage, HyphaCycleInputMode,
    HyphaCycleOutputFailure, HyphaCycleSampleView, HyphaCycleTime, HyphaNode, HyphaPublisher,
    HyphaPublisherOptions, HyphaQos, HyphaStatus, HyphaSubscriber, HyphaSubscriberOptions,
    HyphaWaitSet, cycle_timer_tick, hypha_clock_disable_sim_time, hypha_clock_enable_sim_time,
    hypha_cycle_bind_input, hypha_cycle_bind_output, hypha_cycle_commit, hypha_cycle_create,
    hypha_cycle_destroy, hypha_cycle_emission_commit, hypha_cycle_emission_reserve,
    hypha_cycle_image_sample, hypha_input_source_create, hypha_input_source_destroy,
    hypha_loan_send, hypha_node_create, hypha_node_default_waitset, hypha_node_destroy,
    hypha_publish_with_qos, hypha_publisher_destroy, hypha_publisher_loan_frame,
    hypha_subscribe_typed, hypha_subscriber_destroy,
};
use hypha_node::safe::subscriber_receive;
use hypha_testkit::unique_name;

const DATA_TYPE: &str = "std_msgs/Counter";
const MD5: &str = "d9d213e8c146065ec958333c617eb1a6";
/// The cycle period; the benchmark ticks by hand.
const PERIOD_NANOS: u64 = 1_000_000;
/// The input and output queue depth; one message is in flight at a time.
const DEPTH: usize = 4;

fn c_text(text: &str) -> CString {
    CString::new(text).expect("no NUL in a benchmark name")
}

const FAILED: HyphaCallbackResult = HyphaCallbackResult { action: HYPHA_ACTION_FAIL, status: 0 };

extern "C" fn relay_step(
    image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    let mut sample = HyphaCycleSampleView {
        payload: ptr::null(),
        payload_len: 0,
        transport_sent_nanos: 0,
        sequence: 0,
        publisher_id: 0,
        publisher_sequence: 0,
    };
    // SAFETY: `image` is live for this callback and `sample` is a local the call writes.
    let read = unsafe { hypha_cycle_image_sample(image, 0, 0, &raw mut sample) };
    if read != HyphaStatus::Ok {
        return FAILED;
    }
    let mut payload = ptr::null_mut();
    // SAFETY: `emission` is live for this callback and `payload` is a local the call writes.
    let reserved =
        unsafe { hypha_cycle_emission_reserve(emission, 0, sample.payload_len, &raw mut payload) };
    if reserved != HyphaStatus::Ok || payload.is_null() {
        return FAILED;
    }
    // SAFETY: sample and reservation each hold `payload_len` bytes, in different buffers.
    unsafe { ptr::copy_nonoverlapping(sample.payload, payload, sample.payload_len) };
    // SAFETY: `emission` is live for this callback and holds the open reservation.
    let committed = unsafe { hypha_cycle_emission_commit(emission) };
    if committed != HyphaStatus::Ok {
        return FAILED;
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

fn pubsub(node: *mut HyphaNode, label: &str) -> (*mut HyphaPublisher, *mut HyphaSubscriber) {
    let topic = c_text(&format!("/{}", unique_name(label)));
    let data_type = c_text(DATA_TYPE);
    let md5 = c_text(MD5);
    let options = HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: HyphaQos::default() };
    let mut publisher = ptr::null_mut();
    // SAFETY: `node` is live; every text comes with its length; the outputs are locals.
    let publish_status = unsafe {
        hypha_publish_with_qos(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            data_type.as_ptr(),
            data_type.as_bytes().len(),
            md5.as_ptr(),
            md5.as_bytes().len(),
            c"".as_ptr(),
            0,
            &raw const options,
            &raw mut publisher,
        )
    };
    assert_eq!(publish_status, HyphaStatus::Ok, "publisher");
    let options = HyphaSubscriberOptions {
        depth: DEPTH,
        no_publisher_warning_after_nanos: 0,
        qos: HyphaQos::default(),
    };
    let mut subscriber = ptr::null_mut();
    // SAFETY: as above; a null type name with length 0 accepts the publisher's type.
    let subscribe_status = unsafe {
        hypha_subscribe_typed(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            ptr::null(),
            0,
            md5.as_ptr(),
            md5.as_bytes().len(),
            ptr::null(),
            0,
            &raw const options,
            &raw mut subscriber,
        )
    };
    assert_eq!(subscribe_status, HyphaStatus::Ok, "subscriber");
    (publisher, subscriber)
}

/// One node with a committed relay cycle, its input publisher and output subscriber.
struct Harness {
    node: *mut HyphaNode,
    cycle: *mut HyphaCycle,
    source: *mut hypha_ffi::HyphaInputSource,
    input: *mut HyphaPublisher,
    output: *mut HyphaSubscriber,
}

impl Harness {
    fn new() -> Self {
        let name = c_text(&unique_name("bench_cycle"));
        let mut node = ptr::null_mut();
        // SAFETY: the name comes with its length; null parameters and environment are accepted.
        let created = unsafe {
            hypha_node_create(
                name.as_ptr(),
                name.as_bytes().len(),
                ptr::null(),
                0,
                ptr::null(),
                &raw mut node,
            )
        };
        assert_eq!(created, HyphaStatus::Ok, "node");
        let mut waitset: *mut HyphaWaitSet = ptr::null_mut();
        // SAFETY: `node` is live and `waitset` is a local.
        let lent = unsafe { hypha_node_default_waitset(node, &raw mut waitset) };
        assert_eq!(lent, HyphaStatus::Ok, "waitset");
        let (input, input_subscriber) = pubsub(node, "bench_cycle_in");
        let (output_publisher, output) = pubsub(node, "bench_cycle_out");

        let mut source = ptr::null_mut();
        // SAFETY: `waitset` and `input_subscriber` are live; success takes the subscriber.
        let sourced =
            unsafe { hypha_input_source_create(waitset, input_subscriber, &raw mut source) };
        assert_eq!(sourced, HyphaStatus::Ok, "input source");
        let cycle_name = c_text("bench_relay");
        let mut cycle = ptr::null_mut();
        // SAFETY: `waitset` is live; the step uses no context, so a null context is valid.
        let created = unsafe {
            hypha_cycle_create(
                waitset,
                cycle_name.as_ptr(),
                cycle_name.as_bytes().len(),
                PERIOD_NANOS,
                Some(relay_step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        };
        assert_eq!(created, HyphaStatus::Ok, "cycle");
        let binding = HyphaCycleBindingOptions {
            mode: HyphaCycleInputMode::Latest as u32,
            max_samples_per_iteration: 1,
            optional: 0,
        };
        let mut index = 0_u32;
        // SAFETY: `cycle` and `source` are live; `binding` and `index` are locals.
        let bound =
            unsafe { hypha_cycle_bind_input(cycle, source, &raw const binding, &raw mut index) };
        assert_eq!(bound, HyphaStatus::Ok, "input binding");
        // SAFETY: `cycle` and `output_publisher` are live; success takes the publisher.
        let bound = unsafe {
            hypha_cycle_bind_output(
                cycle,
                output_publisher,
                HyphaCycleOutputFailure::FailRun as u32,
                1,
                &raw mut index,
            )
        };
        assert_eq!(bound, HyphaStatus::Ok, "output binding");
        // SAFETY: `cycle` is live.
        let committed = unsafe { hypha_cycle_commit(cycle) };
        assert_eq!(committed, HyphaStatus::Ok, "commit");
        Self { node, cycle, source, input, output }
    }

    /// Publish `value` on the input, tick once, and take the one output message.
    fn relay(&self, value: u32) {
        let bytes = value.to_le_bytes();
        let mut loan = ptr::null_mut();
        let mut payload = ptr::null_mut();
        // SAFETY: `input` is live; `loan` and `payload` are locals.
        let loaned = unsafe {
            hypha_publisher_loan_frame(self.input, bytes.len(), &raw mut loan, &raw mut payload)
        };
        assert_eq!(loaned, HyphaStatus::Ok, "loan");
        // SAFETY: the loan holds `bytes.len()` writable bytes at `payload` until its send.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), payload, bytes.len()) };
        // SAFETY: `loan` is the live loan taken above; the send consumes it.
        let sent = unsafe { hypha_loan_send(loan, -1) };
        assert_eq!(sent, HyphaStatus::Ok, "send");

        // SAFETY: `cycle` is the live committed cycle, and no `WaitSet` runs meanwhile.
        let ticked = unsafe { cycle_timer_tick(0, self.cycle.cast::<c_void>()) };
        assert_eq!(ticked.action, HYPHA_ACTION_CONTINUE, "the step relays the sample");

        // SAFETY: `output` is a live subscriber handle, the runtime's own subscriber.
        let subscriber = unsafe { self.output.as_ref() }.expect("a live output subscriber");
        let received = subscriber_receive(subscriber).expect("the output frame is accepted");
        assert!(received.is_some(), "the tick sent one message");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // SAFETY: each handle is live and destroyed once, the cycle before its source.
        let destroyed = unsafe { hypha_cycle_destroy(self.cycle) };
        assert_eq!(destroyed, HyphaStatus::Ok, "cycle destroy");
        // SAFETY: as above.
        let destroyed = unsafe { hypha_input_source_destroy(self.source) };
        assert_eq!(destroyed, HyphaStatus::Ok, "source destroy");
        // SAFETY: as above.
        unsafe { hypha_subscriber_destroy(self.output) };
        // SAFETY: as above.
        unsafe { hypha_publisher_destroy(self.input) };
        // SAFETY: as above, after every endpoint of the node.
        unsafe { hypha_node_destroy(self.node) };
    }
}

fn tick_one_input_one_output(c: &mut Criterion) {
    let harness = Harness::new();
    let mut value: u32 = 0;
    c.bench_function("cycle/tick_one_input_one_output", |b| {
        b.iter(|| {
            value = value.wrapping_add(1);
            harness.relay(value);
        });
    });
}

/// The idle inputs of [`sim_turn_idle_inputs`].
const IDLE_INPUTS: usize = 32;

extern "C" fn idle_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// One `WaitSet` turn under simulated time that runs no step: a 1 s replay cycle with
/// [`IDLE_INPUTS`] `Latest` inputs, none of which holds a sample, and a clock that never
/// reaches its first deadline. The turn drains every input once.
fn sim_turn_idle_inputs(c: &mut Criterion) {
    assert_eq!(hypha_clock_enable_sim_time(1_000), HyphaStatus::Ok as i32, "simulated time");
    let name = c_text(&unique_name("bench_sim_turn"));
    let mut node = ptr::null_mut();
    // SAFETY: the name comes with its length; null parameters and environment are accepted.
    let created = unsafe {
        hypha_node_create(
            name.as_ptr(),
            name.as_bytes().len(),
            ptr::null(),
            0,
            ptr::null(),
            &raw mut node,
        )
    };
    assert_eq!(created, HyphaStatus::Ok, "node");
    let mut waitset: *mut HyphaWaitSet = ptr::null_mut();
    // SAFETY: `node` is live and `waitset` is a local.
    let lent = unsafe { hypha_node_default_waitset(node, &raw mut waitset) };
    assert_eq!(lent, HyphaStatus::Ok, "waitset");
    let cycle_name = c_text("bench_idle");
    let mut cycle = ptr::null_mut();
    // SAFETY: `waitset` is live; the step uses no context, so a null context is valid.
    let created = unsafe {
        hypha_cycle_create(
            waitset,
            cycle_name.as_ptr(),
            cycle_name.as_bytes().len(),
            1_000_000_000,
            Some(idle_step),
            ptr::null_mut(),
            None,
            &raw mut cycle,
        )
    };
    assert_eq!(created, HyphaStatus::Ok, "cycle");
    let binding = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 0,
    };
    let mut endpoints = Vec::with_capacity(IDLE_INPUTS);
    for _ in 0..IDLE_INPUTS {
        let (publisher, subscriber) = pubsub(node, "bench_sim_in");
        let mut source = ptr::null_mut();
        // SAFETY: `waitset` and `subscriber` are live; success takes the subscriber.
        let sourced = unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) };
        assert_eq!(sourced, HyphaStatus::Ok, "input source");
        let mut index = 0_u32;
        // SAFETY: `cycle` and `source` are live; `binding` and `index` are locals.
        let bound =
            unsafe { hypha_cycle_bind_input(cycle, source, &raw const binding, &raw mut index) };
        assert_eq!(bound, HyphaStatus::Ok, "input binding");
        endpoints.push((publisher, source));
    }
    // SAFETY: `cycle` is live.
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok, "commit");
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    c.bench_function("cycle/sim_turn_32_idle_inputs", |b| {
        b.iter(|| {
            // SAFETY: `waitset` is live and `report` is a local; a 1 ns run is one turn.
            let ran = unsafe { hypha_waitset_run(waitset, 1, &raw mut report) };
            assert_eq!((ran, report.status), (HyphaStatus::Ok, 0), "the turn runs");
        });
    });
    // SAFETY: each handle is live and destroyed once, the cycle before its sources.
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok, "cycle destroy");
    for (publisher, source) in endpoints {
        // SAFETY: as above.
        let destroyed = unsafe { hypha_input_source_destroy(source) };
        assert_eq!(destroyed, HyphaStatus::Ok, "source destroy");
        // SAFETY: as above.
        unsafe { hypha_publisher_destroy(publisher) };
    }
    // SAFETY: as above, after every endpoint of the node.
    unsafe { hypha_node_destroy(node) };
    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32, "host time");
}

criterion_group!(benches, tick_one_input_one_output, sim_turn_idle_inputs);
criterion_main!(benches);
