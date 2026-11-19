//! Faithful replay under simulated time, through the C ABI.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

mod common;
use crate::common::attach;
use crate::common::emit_message;
use crate::common::replay::*;
use crate::common::{c_text, create_node, unique};

use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

#[test]
fn an_input_600_ms_old_in_data_time_is_stale_under_a_500_ms_limit() {
    let _clock = SimulatedClock::start(0);
    let rig =
        Rig::new("replay_age", HyphaCycleInputMode::Latest, 500 * MILLISECOND as u64, (16, 1024));
    // Sent at 0.4 s, received at once: a host-time judgement would call it fresh.
    unsafe { send_stamped(rig.publisher, 7, 400 * MILLISECOND) };
    set_clock(SECOND);
    rig.settle();

    let at_deadline = rig
        .steps()
        .into_iter()
        .find(|step| step.scheduled == SECOND)
        .expect("the 1.0 s deadline ran");
    assert_eq!(at_deadline.verdict, HYPHA_CYCLE_VERDICT_STALE, "600 ms old in data time");
    assert_eq!(at_deadline.sample_count, 0);
}

#[test]
fn an_input_480_ms_old_in_data_time_is_usable() {
    let _clock = SimulatedClock::start(0);
    let rig = Rig::new(
        "replay_age_ok",
        HyphaCycleInputMode::Latest,
        500 * MILLISECOND as u64,
        (16, 1024),
    );
    unsafe { send_stamped(rig.publisher, 9, 520 * MILLISECOND) };
    set_clock(SECOND);
    rig.settle();

    let at_deadline = rig
        .steps()
        .into_iter()
        .find(|step| step.scheduled == SECOND)
        .expect("the 1.0 s deadline ran");
    assert_eq!(at_deadline.verdict, HYPHA_CYCLE_VERDICT_USABLE, "480 ms old in data time");
    assert_eq!(at_deadline.newest, Some(9));
}

#[test]
fn a_step_takes_only_the_samples_stamped_before_its_deadline() {
    let _clock = SimulatedClock::start(0);
    let rig = Rig::new("replay_cut", HyphaCycleInputMode::Latest, 0, (16, 1024));
    // Sent at the deadline 40 ms: not before it, so the step at 40 ms sees 30 ms.
    unsafe {
        send_stamped(rig.publisher, 1, 30 * MILLISECOND);
        send_stamped(rig.publisher, 2, 40 * MILLISECOND);
    }
    set_clock(40 * MILLISECOND);
    rig.settle();
    let steps = rig.steps();
    let at_40 = steps.iter().find(|step| step.scheduled == 40 * MILLISECOND).copied();
    assert_eq!(at_40.and_then(|step| step.newest), Some(1));
    let at_60 = {
        set_clock(60 * MILLISECOND);
        rig.settle();
        rig.steps().iter().find(|step| step.scheduled == 60 * MILLISECOND).copied()
    };
    assert_eq!(at_60.and_then(|step| step.newest), Some(2));
}

/// The framework send stamps of the outputs a replay step sends.
struct OutputStamps {
    stamps: Mutex<Vec<i64>>,
}

extern "C" fn record_output_stamp(
    delivery: *const HyphaDelivery,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let stamps = unsafe { &*context.cast::<OutputStamps>() };
    let stamp = unsafe { (*delivery).identity.framework_sent_nanos };
    stamps.stamps.lock().expect("stamps").push(i64::try_from(stamp).expect("stamp"));
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// Emits one byte per step on output 0.
extern "C" fn emit_each_step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> HyphaCallbackResult {
    let byte = [1_u8];
    assert_eq!(unsafe { emit_message(emission, 0, &byte) }, HyphaStatus::Ok);
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_replay_steps_outputs_carry_their_deadline_as_the_send_stamp() {
    let clock = SimulatedClock::start(0);
    let node = create_node(&unique("replay_outputs"));
    let waitset = unsafe { create_waitset(node) };
    let topic = format!("/{}", unique("replay_out"));
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 16) };

    let name = c_text(&unique("emitting_cycle"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                u64::try_from(PERIOD_NANOS).expect("period"),
                Some(emit_each_step),
                ptr::null_mut(),
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
                1,
                &raw mut output,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    let stamps = Box::new(OutputStamps { stamps: Mutex::new(Vec::new()) });
    let mut attachment: HyphaAttachmentId = 0;
    assert_eq!(
        unsafe {
            hypha_waitset_attach(
                waitset,
                &attach::subscription(
                    subscriber,
                    Some(record_output_stamp),
                    (&raw const *stamps).cast_mut().cast(),
                    None,
                ),
                &raw mut attachment,
            )
        },
        HyphaStatus::Ok
    );

    // Five deadlines, 20 ms to 100 ms, all run in one turn of the clock.
    set_clock(5 * PERIOD_NANOS);
    run_for(waitset, 50_000_000);
    run_for(waitset, 20_000_000);
    drop(clock);

    let seen = stamps.stamps.lock().expect("stamps").clone();
    let deadlines: Vec<i64> = (1..=5).map(|index| index * PERIOD_NANOS).collect();
    assert_eq!(seen, deadlines, "each output is stamped with the deadline of its step");

    // The cycle owns its output publisher. The subscription detaches before its
    // subscriber is destroyed.
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_waitset_detach(waitset, attachment) }, HyphaStatus::Ok);
    unsafe {
        hypha_subscriber_destroy(subscriber);
        hypha_node_destroy(node);
    }
}
