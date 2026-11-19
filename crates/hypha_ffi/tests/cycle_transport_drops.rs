//! Transport drops at a cycle input: the held report says the count is unknown.
#![allow(clippy::expect_used, clippy::too_many_lines)]

mod common;
use crate::common::c_text;
use crate::common::create_node;
use crate::common::unique;

use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, PoisonError};

use hypha_ffi::waitset::*;
use hypha_ffi::*;

static SERIAL: Mutex<()> = Mutex::new(());

unsafe fn create_waitset(node: *mut HyphaNode) -> *mut HyphaWaitSet {
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    waitset
}

unsafe fn create_pubsub(
    node: *mut HyphaNode,
    topic_str: &str,
) -> (*mut HyphaPublisher, *mut HyphaSubscriber) {
    let topic = c_text(topic_str);
    let data_type = c_text("std_msgs/Counter");
    let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
    let definition = c_text("");

    let pub_qos =
        HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
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
        depth: 4,
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
                std::ptr::null(),
                0,
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

unsafe fn send_sample(publisher: *mut HyphaPublisher, data: &[u8]) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_publisher_loan_frame(publisher, data.len(), &raw mut loan, &raw mut bytes) },
        HyphaStatus::Ok
    );
    unsafe { ptr::copy_nonoverlapping(data.as_ptr(), bytes, data.len()) };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
}

fn run_waitset_for(waitset: *mut HyphaWaitSet, nanos: u64) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    let _ = unsafe { hypha_waitset_run(waitset, nanos, &raw mut report) };
}

extern "C" fn held_report_step(
    image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let reports = unsafe { &*context.cast::<Mutex<Vec<HyphaCycleHeldReport>>>() };
    let mut binding = HyphaCycleBindingView::default();
    if unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) } == HyphaStatus::Ok
        && let Ok(mut all) = reports.lock()
    {
        all.push(binding.held);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[derive(Debug, Clone, Copy)]
struct PolicyObservation {
    report: HyphaCycleBindingReport,
    first: u8,
    last: u8,
}

extern "C" fn policy_report_step(
    image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let observations = unsafe { &*context.cast::<Mutex<Vec<PolicyObservation>>>() };
    let mut binding = HyphaCycleBindingView::default();
    if unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) } == HyphaStatus::Ok
        && binding.health.capture.sample_count > 0
    {
        let report = binding.health.capture;
        let mut first = HyphaCycleSampleView {
            payload: ptr::null(),
            payload_len: 0,
            transport_sent_nanos: 0,
            sequence: 0,
            publisher_id: 0,
            publisher_sequence: 0,
        };
        let last_index = report.sample_count.saturating_sub(1);
        let mut last = HyphaCycleSampleView {
            payload: ptr::null(),
            payload_len: 0,
            transport_sent_nanos: 0,
            sequence: 0,
            publisher_id: 0,
            publisher_sequence: 0,
        };
        let first_status = unsafe { hypha_cycle_image_sample(image, 0, 0, &raw mut first) };
        let last_status = unsafe { hypha_cycle_image_sample(image, 0, last_index, &raw mut last) };
        if first_status == HyphaStatus::Ok && last_status == HyphaStatus::Ok {
            let first_byte = unsafe { *first.payload };
            let last_byte = unsafe { *last.payload };
            if let Ok(mut all) = observations.lock() {
                all.push(PolicyObservation { report, first: first_byte, last: last_byte });
            }
        }
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_burst_beyond_the_transport_depth_shows_as_overruns() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("cyc_drops"));
    let waitset = unsafe { create_waitset(node) };
    let in_topic = format!("/{}", unique("drops_in"));
    let (pub_in, sub_in) = unsafe { create_pubsub(node, &in_topic) };

    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub_in, &raw mut source) },
        HyphaStatus::Ok
    );
    let reports: Box<Mutex<Vec<HyphaCycleHeldReport>>> = Box::new(Mutex::new(Vec::new()));
    let name = c_text("drops_cycle");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000,
                Some(held_report_step),
                (&raw const *reports).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let bind = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: 64,
        optional: 0,
    };
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    // Establish a received sequence before overflowing the transport, so a later sequence
    // gap is observable.
    unsafe { send_sample(pub_in, &[0]) };
    run_waitset_for(waitset, 30_000_000);

    // Sixteen back-to-back samples, more than the subscriber depth of four.
    for sample in 0..16_u8 {
        unsafe { send_sample(pub_in, &[sample]) };
    }
    run_waitset_for(waitset, 300_000_000);

    let all = reports.lock().expect("reports");
    let last = all.last().copied().expect("the step ran");
    assert!(last.received_total < 17, "the burst did not exceed the transport: {last:?}");
    drop(all);

    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, 0, &raw mut binding) }, HyphaStatus::Ok);
    let health = binding.health;
    assert!(health.overrun_total > 0, "sequence gaps must be visible as overruns: {health:?}");

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(pub_in);
        hypha_node_destroy(node);
    }
}

#[test]
fn a_latest_binding_is_not_charged_for_superseded_transport_gaps() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let node = create_node(&unique("cyc_latest_drops"));
    let waitset = unsafe { create_waitset(node) };
    let in_topic = format!("/{}", unique("latest_drops_in"));
    let (pub_in, sub_in) = unsafe { create_pubsub(node, &in_topic) };

    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, sub_in, &raw mut source) },
        HyphaStatus::Ok
    );
    let observations: Box<Mutex<Vec<PolicyObservation>>> = Box::default();
    let name = c_text("latest_drops_cycle");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                10_000_000,
                Some(policy_report_step),
                (&raw const *observations).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let bind = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Latest as u32,
        max_samples_per_iteration: 1,
        optional: 0,
    };
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    // The same burst as the queued case: sixteen samples through a depth-four transport.
    unsafe { send_sample(pub_in, &[0]) };
    run_waitset_for(waitset, 30_000_000);
    for sample in 0..16_u8 {
        unsafe { send_sample(pub_in, &[sample]) };
    }
    run_waitset_for(waitset, 300_000_000);

    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, 0, &raw mut binding) }, HyphaStatus::Ok);
    let health = binding.health;
    assert_eq!(
        health.overrun_total, 0,
        "superseded samples are not lost to a latest binding: {health:?}"
    );

    let all = observations.lock().expect("observations");
    let last = all.last().copied().expect("the step captured the newest value");
    assert_eq!(last.report.sample_count, 1, "a latest binding holds one value: {last:?}");
    assert_eq!(last.last, 15, "the newest sample is delivered: {last:?}");
    drop(all);

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(pub_in);
        hypha_node_destroy(node);
    }
}

#[test]
fn tick_drain_drops_the_oldest_excess_of_a_queued_input() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);

    // A queued input takes its share of the newest samples and drops the rest.
    let (expected_first, expected_last, expected_backlog, expected_drops) = (3, 4, 0, 2);
    let node = create_node(&unique("cyc_policy"));
    let waitset = unsafe { create_waitset(node) };
    let in_topic = format!("/{}", unique("policy_in"));
    let (publisher, subscriber) = unsafe { create_pubsub(node, &in_topic) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );

    let observations: Box<Mutex<Vec<PolicyObservation>>> = Box::default();
    let name = c_text("policy_cycle");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                50_000_000,
                Some(policy_report_step),
                (&raw const *observations).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let binding = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: 2,
        optional: 0,
    };
    let mut index = 0;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const binding, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    // Fill the transport queue before the first 50 ms tick drains it.
    for sample in 1..=4_u8 {
        unsafe { send_sample(publisher, &[sample]) };
    }
    run_waitset_for(waitset, 120_000_000);
    let first =
        *observations.lock().expect("observations").first().expect("the step captured a batch");
    assert_eq!(first.report.verdict, HYPHA_CYCLE_VERDICT_USABLE);
    assert_eq!(first.report.sample_count, 2);
    assert_eq!(first.report.backlog_samples, expected_backlog);
    assert_eq!(first.report.work_limit_drops, expected_drops);
    assert_eq!((first.first, first.last), (expected_first, expected_last));

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
}
