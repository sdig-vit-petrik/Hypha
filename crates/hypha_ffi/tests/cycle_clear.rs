//! A step clears its inputs through the C ABI (#213), on simulated time: the clear at a
//! step forgets what that step's cut admitted, the same in every run and every chunking.
#![allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]

mod common;
use crate::common::replay::*;
use crate::common::{c_text, create_node, unique};

use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

/// The step that clears both inputs.
const CLEAR_AT: i64 = 100 * MILLISECOND;

/// What one step saw: its deadline in ms, then for the latest input (binding 0) its
/// verdict and held value, and for the queued one (binding 1) its verdict and samples.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    at_ms: i64,
    latest: (u32, Option<u8>),
    queued: (u32, Vec<u8>),
}

#[derive(Default)]
struct Steps {
    seen: Mutex<Vec<Seen>>,
    refusals: Mutex<Vec<HyphaStatus>>,
}

fn verdict(image: *const HyphaCycleImage, binding: u32) -> u32 {
    let mut view = HyphaCycleBindingView::default();
    assert_eq!(
        unsafe { hypha_cycle_image_binding(image, binding, &raw mut view) },
        HyphaStatus::Ok
    );
    view.health.capture.verdict
}

fn empty_view() -> HyphaCycleSampleView {
    HyphaCycleSampleView {
        payload: ptr::null(),
        payload_len: 0,
        transport_sent_nanos: 0,
        sequence: 0,
        publisher_id: 0,
        publisher_sequence: 0,
    }
}

extern "C" fn clearing_step(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let steps = unsafe { &*context.cast::<Steps>() };
    let scheduled = unsafe { (*time).scheduled_nanos };
    let mut held = empty_view();
    let has_held = unsafe { hypha_cycle_image_held(image, 0, &raw mut held) } == HyphaStatus::Ok;
    let latest = (verdict(image, 0), has_held.then(|| unsafe { *held.payload }));
    let mut samples = Vec::new();
    let mut index = 0;
    loop {
        let mut view = empty_view();
        if unsafe { hypha_cycle_image_sample(image, 1, index, &raw mut view) } != HyphaStatus::Ok {
            break;
        }
        samples.push(unsafe { *view.payload });
        index += 1;
    }
    let seen =
        Seen { at_ms: scheduled / MILLISECOND, latest, queued: (verdict(image, 1), samples) };
    steps.seen.lock().unwrap().push(seen);
    if scheduled == CLEAR_AT {
        for binding in [0, 1] {
            let status =
                unsafe { hypha_cycle_image_clear(image, HYPHA_CYCLE_CLEAR_INPUT, binding) };
            assert_eq!(status, HyphaStatus::Ok);
        }
        let mut refusals = steps.refusals.lock().unwrap();
        refusals.push(unsafe { hypha_cycle_image_clear(image, HYPHA_CYCLE_CLEAR_INPUT, 2) });
        refusals.push(unsafe { hypha_cycle_image_clear(image, HYPHA_CYCLE_CLEAR_TRANSFORMS, 0) });
        refusals.push(unsafe { hypha_cycle_image_clear(image, 7, 0) });
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// A 20 ms cycle over a 100 Hz topic stamped 10..=300 ms, read by a latest and a queued
/// binding of one source. `ahead_ms` is how far past each `/clock` the samples are sent:
/// 0 as a player that closes each chunk at its clock, more as one that sends a chunk
/// ahead. `close_ms` is the clock step. No sample is stamped in the range `gap_ms`.
fn replay(
    close_ms: i64,
    ahead_ms: i64,
    gap_ms: std::ops::Range<i64>,
) -> (Vec<Seen>, HyphaCycleBindingView, Vec<HyphaStatus>) {
    let _clock = SimulatedClock::start(0);
    let node = create_node(&unique("cycle_clear"));
    let waitset = unsafe { create_waitset(node) };
    let topic = format!("/{}", unique("cycle_clear"));
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 32) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let steps = Box::new(Steps::default());
    let name = c_text(&unique("clear_cycle"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                u64::try_from(PERIOD_NANOS).expect("period"),
                Some(clearing_step),
                (&raw const *steps).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    for (mode, per_step) in [(HyphaCycleInputMode::Latest, 1), (HyphaCycleInputMode::Queued, 32)] {
        let bind = HyphaCycleBindingOptions {
            mode: mode as u32,
            max_samples_per_iteration: per_step,
            optional: 0,
        };
        let mut index = 0_u32;
        assert_eq!(
            unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
            HyphaStatus::Ok
        );
    }
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);

    let mut next = 10;
    // The last clock, 320 ms, is the step after the last sample, in every clock step.
    let closes = (1..).map(|n| (n * close_ms).min(320)).take_while(|&close| close < 320);
    for close in closes.chain([320]) {
        while next <= (close + ahead_ms).min(300) {
            if !gap_ms.contains(&next) {
                let value = u8::try_from(next / 10).expect("value");
                unsafe { send_stamped(publisher, value, next * MILLISECOND) };
            }
            next += 10;
        }
        set_clock(close * MILLISECOND);
        assert_eq!(run_status(waitset, 5_000_000), HyphaStatus::Ok as i32);
    }

    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, 1, &raw mut binding) }, HyphaStatus::Ok);
    let seen = steps.seen.lock().unwrap().clone();
    let refusals = steps.refusals.lock().unwrap().clone();
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_input_source_destroy(source) }, HyphaStatus::Ok);
    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(node);
    }
    (seen, binding, refusals)
}

#[test]
fn a_replay_clear_forgets_what_its_step_admitted_in_every_run_and_chunking() {
    let reference = replay(100, 0, 0..0);
    assert_eq!(reference, replay(100, 0, 0..0), "a second run");
    assert_eq!(reference, replay(20, 0, 0..0), "a 20 ms clock step");
    assert_eq!(reference, replay(100, 100, 0..0), "a chunk sent ahead of its clock");
    assert_eq!(reference, replay(20, 300, 0..0), "every sample before the first clock");

    let (seen, binding, refusals) = reference;
    let health = binding.health;
    let at = |ms: i64| seen.iter().find(|step| step.at_ms == ms).cloned().expect("a step");
    assert_eq!(at(100).latest, (HYPHA_CYCLE_VERDICT_USABLE, Some(9)));
    assert_eq!(at(100).queued, (HYPHA_CYCLE_VERDICT_USABLE, vec![8, 9]));
    // 100 and 110 ms were sent at and after the clear's cut of 100 ms: they stay.
    assert_eq!(at(120).latest, (HYPHA_CYCLE_VERDICT_USABLE, Some(11)));
    assert_eq!(at(120).queued, (HYPHA_CYCLE_VERDICT_USABLE, vec![10, 11]));
    // A rate step takes every sample its cut admits, so the clear found none pending.
    assert_eq!(health.cleared_total, 0, "{health:?}");
    assert_eq!(health.overrun_total, 0, "{health:?}");
    let refused = HyphaStatus::InvalidArgument;
    assert_eq!(refusals, [refused, refused, refused], "an unknown binding, TF input and kind");
}
