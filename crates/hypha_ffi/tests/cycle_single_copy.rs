//! Single-copy emission, observed on the bus: a subscriber receives the bytes the step
//! wrote, in the order the step staged them.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::unwrap_used
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

/// The payloads one step stages, in order, on output 0.
struct Messages {
    payloads: Vec<Vec<u8>>,
}

extern "C" fn stage_messages(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let messages = unsafe { &*context.cast::<Messages>() };
    for payload in &messages.payloads {
        assert_eq!(unsafe { emit_message(emission, 0, payload) }, HyphaStatus::Ok);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// Every payload and transport sequence the subscriber receives.
struct Received {
    payloads: Mutex<Vec<(Vec<u8>, u64)>>,
}

extern "C" fn record_payload(
    delivery: *const HyphaDelivery,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let received = unsafe { &*context.cast::<Received>() };
    let delivery = unsafe { &*delivery };
    let payload = unsafe { std::slice::from_raw_parts(delivery.payload, delivery.payload_len) };
    let transport_sent = delivery.identity.transport_sent_nanos;
    received.payloads.lock().expect("received").push((payload.to_vec(), transport_sent));
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn staged_bytes_reach_the_subscriber_unchanged_and_in_staging_order() {
    let node = create_node(&unique("single_copy"));
    let mut waitset: *mut HyphaWaitSet = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);

    // Three messages per step, so the publisher holds three loans.
    let topic = c_text(&format!("/{}", unique("single_copy_out")));
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

    // Empty, one byte and a longer payload: each must arrive with its own length.
    let staged = vec![b"alpha".to_vec(), Vec::new(), b"gamma-payload-of-some-length".to_vec()];
    let messages = Box::new(Messages { payloads: staged.clone() });
    let name = c_text(&unique("single_copy_cycle"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                PERIOD_NANOS,
                Some(stage_messages),
                (&raw const *messages).cast_mut().cast(),
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
        if received.payloads.lock().expect("received").len() >= staged.len() {
            break;
        }
    }

    let seen = received.payloads.lock().expect("received").clone();
    assert!(seen.len() >= staged.len(), "at least one full step was delivered, saw {}", seen.len());
    // Every step stages the same three messages, so the stream repeats them in order.
    for (position, (payload, _)) in seen.iter().enumerate() {
        let expected = staged.get(position % staged.len()).expect("a staged message");
        assert_eq!(payload, expected, "message {position} is unchanged");
    }
    // Sent in order within a step: transport stamps never run backwards.
    let stamps: Vec<u64> = seen.iter().map(|(_, stamp)| *stamp).collect();
    assert!(stamps.windows(2).all(|pair| matches!(pair, [earlier, later] if earlier <= later)));

    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    // The WaitSet releases the subscription before the subscriber it borrows is destroyed.
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_node_destroy(node);
    }
    drop(messages);
    drop(received);
}
