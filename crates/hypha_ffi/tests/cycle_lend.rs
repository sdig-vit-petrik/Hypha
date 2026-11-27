//! A cycle step lends output slots and fills them in place (`hypha_cycle_emission_lend`):
//! several at once beside a reserve, sent shrunk in the order of the sends, a returned loan
//! sends nothing and gives its count back, and an unsent one goes back at the step's end.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]

mod common;
use crate::common::attach;
use crate::common::emit_message;
use crate::common::replay::run_for;
use crate::common::{c_text, create_node, unique};

use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

const PERIOD_NANOS: u64 = 10_000_000;

/// The statuses the first step saw, in order.
#[derive(Default)]
struct Step {
    statuses: Mutex<Vec<HyphaStatus>>,
    steps: std::sync::atomic::AtomicU32,
}

unsafe fn lend(emission: *mut HyphaCycleEmission, capacity: usize, fill: u8) -> (HyphaStatus, u64) {
    let mut bytes = ptr::null_mut();
    let mut token = 0;
    let status =
        unsafe { hypha_cycle_emission_lend(emission, 0, capacity, &raw mut bytes, &raw mut token) };
    if status == HyphaStatus::Ok {
        let slot = unsafe { std::slice::from_raw_parts_mut(bytes, capacity) };
        assert!(slot.iter().all(|byte| *byte == 0), "a lent slot holds zeros");
        slot.fill(fill);
    }
    (status, token)
}

extern "C" fn lend_in_place(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let step = unsafe { &*context.cast::<Step>() };
    if step.steps.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
        // Later steps lend and keep a loan unsent: the step's end gives it back.
        let _kept = unsafe { lend(emission, 8, 0x55) };
        return HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 };
    }
    let mut seen = Vec::new();
    let (a, a_token) = unsafe { lend(emission, 100, 0xAA) };
    let (b, b_token) = unsafe { lend(emission, 50, 0xBB) };
    seen.extend([a, b]);
    // The output's count is 3: a third lend fits, a fourth is refused and latches nothing.
    let (c, c_token) = unsafe { lend(emission, 10, 0xCC) };
    let (d, _) = unsafe { lend(emission, 10, 0xDD) };
    seen.extend([c, d]);
    // A returned loan gives its count back: the reserve fits after it.
    seen.push(unsafe { hypha_cycle_emission_return(emission, c_token) });
    seen.push(unsafe { emit_message(emission, 0, b"reserved") });
    // Sent shrunk, in the order of the sends: B, the reserve went first, then A.
    seen.push(unsafe { hypha_cycle_emission_send(emission, b_token, 5) });
    seen.push(unsafe { hypha_cycle_emission_send(emission, a_token, 20) });
    // A token sent already, and a length past the loan, are refused.
    seen.push(unsafe { hypha_cycle_emission_send(emission, a_token, 1) });
    step.statuses.lock().expect("statuses").extend(seen);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

struct Received {
    payloads: Mutex<Vec<Vec<u8>>>,
}

extern "C" fn record_payload(
    delivery: *const HyphaDelivery,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let received = unsafe { &*context.cast::<Received>() };
    let delivery = unsafe { &*delivery };
    let payload = unsafe { std::slice::from_raw_parts(delivery.payload, delivery.payload_len) };
    received.payloads.lock().expect("received").push(payload.to_vec());
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn lent_slots_are_sent_shrunk_in_the_order_of_the_sends() {
    let node = create_node(&unique("cycle_lend"));
    let mut waitset: *mut HyphaWaitSet = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);

    // Three messages per step, so the publisher holds three loans.
    let topic = c_text(&format!("/{}", unique("cycle_lend_out")));
    let data_type = c_text("std_msgs/Bytes");
    let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
    let definition = c_text("");
    let pub_qos =
        HyphaPublisherOptions { latch: 0, loaned_samples: 3, qos: crate::common::test_qos() };
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
        depth: 8,
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
                ptr::null(),
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

    let step = Box::new(Step::default());
    let name = c_text(&unique("cycle_lend_cycle"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                PERIOD_NANOS,
                Some(lend_in_place),
                (&raw const *step).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let mut output = 0_u32;
    assert_eq!(
        unsafe {
            hypha_cycle_bind_output(
                cycle,
                publisher,
                HyphaCycleOutputFailure::FailRun as u32,
                3,
                &raw mut output,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    let received = Box::new(Received { payloads: Mutex::new(Vec::new()) });
    let mut attachment: HyphaAttachmentId = 0;
    assert_eq!(
        unsafe {
            hypha_waitset_attach(
                waitset,
                &attach::subscription(
                    subscriber,
                    Some(record_payload),
                    (&raw const *received).cast_mut().cast(),
                    None,
                ),
                &raw mut attachment,
            )
        },
        HyphaStatus::Ok
    );
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(3) {
        run_for(waitset, 10_000_000);
        if step.steps.load(std::sync::atomic::Ordering::SeqCst) >= 4 {
            break;
        }
    }
    run_for(waitset, 20_000_000);

    let statuses = step.statuses.lock().expect("statuses").clone();
    assert_eq!(
        statuses,
        vec![
            HyphaStatus::Ok,
            HyphaStatus::Ok,
            HyphaStatus::Ok,
            HyphaStatus::ResourceLimit,
            HyphaStatus::Ok,
            HyphaStatus::Ok,
            HyphaStatus::Ok,
            HyphaStatus::Ok,
            HyphaStatus::InvalidArgument,
        ]
    );
    let seen = received.payloads.lock().expect("received").clone();
    assert_eq!(
        seen,
        vec![b"reserved".to_vec(), vec![0xBB; 5], vec![0xAA; 20]],
        "the reserve and the two loans, shrunk, in the order of the sends; nothing of a returned \
         loan nor of a loan left unsent at a step's end"
    );

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_node_destroy(node);
    }
    drop(step);
    drop(received);
}
