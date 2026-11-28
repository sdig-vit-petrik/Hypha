//! A cycle input reads a large message in place, where it arrived: the step's view points
//! into the subscriber's slot, a slot goes back once every binding took its sample, and a
//! sample held past the borrow budget is copied out, counted, with its bytes unchanged.
#![allow(clippy::expect_used, clippy::too_many_lines, clippy::arithmetic_side_effects)]

mod common;
use crate::common::{c_text, create_node, unique};

use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, PoisonError};

use hypha_ffi::waitset::*;
use hypha_ffi::*;
use hypha_node::safe::input_source_in_place;

static SERIAL: Mutex<()> = Mutex::new(());

/// Large enough to be read in place (`PIN_MIN_BYTES` is 64 KiB).
const FRAME_BYTES: usize = 100_000;

/// What each step saw: per sample, its first byte and whether every byte equals it, and
/// how many samples the source read in place while the step ran.
#[derive(Default)]
struct Seen {
    steps: Mutex<Vec<Vec<(u8, bool)>>>,
    in_place_in_step: Mutex<Vec<usize>>,
    source: std::sync::atomic::AtomicPtr<HyphaInputSource>,
    latest: bool,
}

extern "C" fn record(
    image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let seen = unsafe { &*context.cast::<Seen>() };
    let mut binding = HyphaCycleBindingView::default();
    assert_eq!(unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) }, HyphaStatus::Ok);
    let mut step = Vec::new();
    let mut view = HyphaCycleSampleView {
        payload: ptr::null(),
        payload_len: 0,
        transport_sent_nanos: 0,
        sequence: 0,
        publisher_id: 0,
        publisher_sequence: 0,
    };
    for index in 0..binding.health.capture.sample_count {
        assert_eq!(
            unsafe { hypha_cycle_image_sample(image, 0, index, &raw mut view) },
            HyphaStatus::Ok
        );
        let bytes = unsafe { std::slice::from_raw_parts(view.payload, view.payload_len) };
        let first = bytes.first().copied().unwrap_or(0);
        step.push((first, bytes.len() == FRAME_BYTES && bytes.iter().all(|byte| *byte == first)));
    }
    if seen.latest && unsafe { hypha_cycle_image_held(image, 0, &raw mut view) } == HyphaStatus::Ok
    {
        let bytes = unsafe { std::slice::from_raw_parts(view.payload, view.payload_len) };
        let first = bytes.first().copied().unwrap_or(0);
        step.push((first, bytes.len() == FRAME_BYTES && bytes.iter().all(|byte| *byte == first)));
    }
    if !step.is_empty() {
        seen.steps.lock().expect("seen").push(step);
        let source = seen.source.load(std::sync::atomic::Ordering::Acquire);
        let (in_place, _) = input_source_in_place(unsafe { &*source });
        seen.in_place_in_step.lock().expect("seen").push(in_place);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

struct Rig {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    publisher: *mut HyphaPublisher,
    source: *mut HyphaInputSource,
    cycle: *mut HyphaCycle,
    seen: Box<Seen>,
}

unsafe fn rig(mode: HyphaCycleInputMode) -> Rig {
    let mode_value = mode as u32;
    let node = create_node(&unique("in_place"));
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let topic = c_text(&format!("/{}", unique("in_place_cloud")));
    let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
    let qos = crate::common::test_qos();
    let options = HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos };
    let mut publisher = ptr::null_mut();
    let status = unsafe {
        hypha_publish_with_qos(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            c"std_msgs/Bytes".as_ptr(),
            "std_msgs/Bytes".len(),
            md5.as_ptr(),
            32,
            c"".as_ptr(),
            0,
            &raw const options,
            &raw mut publisher,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    let options = HyphaSubscriberOptions { depth: 4, no_publisher_warning_after_nanos: 0, qos };
    let mut subscriber = ptr::null_mut();
    let status = unsafe {
        hypha_subscribe_typed(
            node,
            topic.as_ptr(),
            topic.as_bytes().len(),
            ptr::null(),
            0,
            md5.as_ptr(),
            32,
            ptr::null(),
            0,
            &raw const options,
            &raw mut subscriber,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let seen = Box::new(Seen { latest: mode == HyphaCycleInputMode::Latest, ..Seen::default() });
    seen.source.store(source, std::sync::atomic::Ordering::Release);
    let name = c_text("in_place_cycle");
    let mut cycle = ptr::null_mut();
    let status = unsafe {
        hypha_cycle_create(
            waitset,
            name.as_ptr(),
            name.as_bytes().len(),
            10_000_000,
            Some(record),
            (&raw const *seen).cast_mut().cast(),
            None,
            &raw mut cycle,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    let bind =
        HyphaCycleBindingOptions { mode: mode_value, max_samples_per_iteration: 4, optional: 0 };
    let mut index = 0_u32;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const bind, &raw mut index) },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    Rig { node, waitset, publisher, source, cycle, seen }
}

unsafe fn send(publisher: *mut HyphaPublisher, value: u8) {
    let mut loan = ptr::null_mut();
    let mut bytes = ptr::null_mut();
    let status = unsafe {
        hypha_publisher_loan_frame(publisher, FRAME_BYTES, &raw mut loan, &raw mut bytes)
    };
    assert_eq!(status, HyphaStatus::Ok);
    unsafe { ptr::write_bytes(bytes, value, FRAME_BYTES) };
    assert_eq!(unsafe { hypha_loan_send(loan, -1) }, HyphaStatus::Ok);
}

fn run(waitset: *mut HyphaWaitSet, nanos: u64) {
    let mut report =
        HyphaWaitSetRunReport { reason: 0, status: 0, attachment: 0, slow_callbacks: 0 };
    let _ = unsafe { hypha_waitset_run(waitset, nanos, &raw mut report) };
}

impl Rig {
    fn in_place(&self) -> (usize, u64) {
        input_source_in_place(unsafe { &*self.source })
    }

    fn steps(&self) -> Vec<Vec<(u8, bool)>> {
        std::mem::take(&mut *self.seen.steps.lock().expect("seen"))
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

#[test]
fn arrivals_read_each_large_frame_in_place_and_give_the_slot_back_once_taken() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = unsafe { rig(HyphaCycleInputMode::Queued) };
    for value in 1..=6_u8 {
        unsafe { send(rig.publisher, value) };
        run(rig.waitset, 30_000_000);
        let (in_place, copied) = rig.in_place();
        assert!(in_place <= 1, "frame {value} at most is read in place, the ones before went back");
        assert_eq!(copied, 0, "a frame each step takes is never copied");
    }
    let steps: Vec<(u8, bool)> = rig.steps().into_iter().flatten().collect();
    assert_eq!(steps, (1..=6).map(|value| (value, true)).collect::<Vec<_>>());
    let in_step = std::mem::take(&mut *rig.seen.in_place_in_step.lock().expect("seen"));
    assert_eq!(in_step, vec![1; 6], "each step read its frame where it arrived");
}

#[test]
fn a_burst_past_the_borrow_budget_copies_the_oldest_out_and_keeps_its_bytes() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = unsafe { rig(HyphaCycleInputMode::Queued) };
    for value in [7_u8, 8, 9] {
        unsafe { send(rig.publisher, value) };
    }
    run(rig.waitset, 30_000_000);
    let (_, copied) = rig.in_place();
    assert!(copied >= 1, "two frames fill the budget: the oldest was copied out ({copied})");
    let steps: Vec<(u8, bool)> = rig.steps().into_iter().flatten().collect();
    assert_eq!(steps, vec![(7, true), (8, true), (9, true)], "every byte is the one sent");
}

#[test]
fn a_latest_input_keeps_only_the_newest_frame_in_place() {
    let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let rig = unsafe { rig(HyphaCycleInputMode::Latest) };
    for value in [3_u8, 4, 5] {
        unsafe { send(rig.publisher, value) };
    }
    run(rig.waitset, 30_000_000);
    assert_eq!(rig.in_place(), (1, 0), "the newest frame in place, the others went back");
    let in_step = std::mem::take(&mut *rig.seen.in_place_in_step.lock().expect("seen"));
    assert!(in_step.iter().all(|count| *count == 1), "{in_step:?}");
    let steps = rig.steps();
    assert!(
        !steps.is_empty() && steps.iter().flatten().all(|seen| *seen == (5, true)),
        "{steps:?}"
    );
    unsafe { send(rig.publisher, 6) };
    run(rig.waitset, 30_000_000);
    assert_eq!(rig.in_place(), (1, 0));
    let steps = rig.steps();
    assert!(
        !steps.is_empty() && steps.iter().flatten().all(|seen| *seen == (6, true)),
        "{steps:?}"
    );
}
