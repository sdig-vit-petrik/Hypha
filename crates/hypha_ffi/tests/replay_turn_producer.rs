//! A producer in the same `WaitSet` as a replay cycle, run after the cycle's steps on a
//! turn, publishes into the cycle's input: the turn drains that input again before it
//! reports, so the queue holds one turn of the producer's output.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::unwrap_used
)]

mod common;
use crate::common::replay::*;
use crate::common::{create_node, unique};

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::time::Duration;

use hypha_ffi::waitset::*;
use hypha_ffi::*;

struct Producer {
    publisher: *mut HyphaPublisher,
    stamp: AtomicI64,
    value: AtomicU8,
    per_firing: u8,
}

extern "C" fn produce(_missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    let producer = unsafe { &*context.cast::<Producer>() };
    for _ in 0..producer.per_firing {
        let value = producer.value.fetch_add(1, Ordering::Relaxed);
        unsafe { send_stamped(producer.publisher, value, producer.stamp.load(Ordering::Relaxed)) };
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// A 100 ms replay cycle with a queued input of queue 4, and after it in the `WaitSet` a
/// plain 10 ms timer that publishes 3 frames into that input on each firing. One turn per
/// `/clock` value: 100 (the cycle steps, the timer sends 3), 150 (no step, the timer sends
/// 3 more), 200 (the cycle steps). Returns each turn's status and the steps.
fn in_process_producer_after_the_consumer() -> (Vec<i32>, Vec<Step>, HyphaCycleBindingHealth) {
    let _clock = SimulatedClock::start(10 * MILLISECOND);
    let node = create_node(&unique("turn_producer"));
    let waitset = unsafe { create_waitset(node) };
    let topic = format!("/{}", unique("turn_producer"));
    let (publisher, subscriber) = unsafe { create_pubsub(node, &topic, 4) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let recorder = Box::new(Recorder::new(Duration::ZERO));
    let name = crate::common::c_text(&unique("turn_producer_cycle"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                100_000_000,
                Some(record_step),
                (&raw const *recorder).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let bind = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: 20,
        optional: 0,
    };
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    let producer = Box::new(Producer {
        publisher,
        stamp: AtomicI64::new(0),
        value: AtomicU8::new(10),
        per_firing: 3,
    });
    let mut attachment: HyphaAttachmentId = 0;
    assert_eq!(
        unsafe {
            hypha_waitset_attach(
                waitset,
                &crate::common::attach::timer(
                    10_000_000,
                    Some(produce),
                    (&raw const *producer).cast_mut().cast(),
                    None,
                ),
                &raw mut attachment,
            )
        },
        HyphaStatus::Ok
    );
    // One frame the cycle drains first, so a later overflow is a sequence gap.
    unsafe { send_stamped(publisher, 1, 50 * MILLISECOND) };
    let mut statuses = Vec::new();
    for clock_ms in [100, 150, 200] {
        producer.stamp.store(clock_ms * MILLISECOND - 1, Ordering::Relaxed);
        set_clock(clock_ms * MILLISECOND);
        // A 1 ns run is one turn.
        statuses.push(run_status(waitset, 1));
    }
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_binding(cycle, 0, &raw mut binding) }, HyphaStatus::Ok);
    let steps = recorder.steps();
    unsafe {
        assert_eq!(hypha_cycle_destroy(cycle), HyphaStatus::Ok);
        assert_eq!(hypha_input_source_destroy(source), HyphaStatus::Ok);
        hypha_node_destroy(node);
    }
    drop(producer);
    (statuses, steps, binding.health)
}

#[test]
fn an_in_process_producer_after_a_stepped_cycle_fits_a_queue_of_one_turn() {
    let (statuses, steps, health) = in_process_producer_after_the_consumer();
    let counts: Vec<(i64, usize)> =
        steps.iter().map(|step| (step.scheduled / MILLISECOND, step.sample_count)).collect();
    // The producer sent 3 per turn, a queue of 4 holds one turn of it.
    assert_eq!(statuses, [HyphaStatus::Ok as i32; 3], "{health:?}");
    assert_eq!(counts, [(100, 1), (200, 6)], "{health:?}");
}
