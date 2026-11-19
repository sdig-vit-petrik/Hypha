//! Triggered cycles at the C ABI: refusals at registration and commit, and their
//! registrations on the `WaitSet`.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    reason = "a test asserts on known values and fails loudly"
)]

use super::*;
use crate::common::attach;
use crate::common::supervision::{bind_trigger, node_with_triggers};

const SILENCE_NANOS: i64 = 100_000_000;

static RELEASES: AtomicU32 = AtomicU32::new(0);

extern "C" fn count_release(_context: *mut c_void) {
    RELEASES.fetch_add(1, Ordering::SeqCst);
}

extern "C" fn no_interval(_missed: u64, _context: *mut c_void) -> HyphaCallbackResult {
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// A cycle of `period_nanos` with one input in `mode`, on a fresh notified topic.
/// Returns the cycle, the publisher and the binding index.
unsafe fn cycle_with_input(
    waitset: *mut HyphaWaitSet,
    node: *mut HyphaNode,
    period_nanos: u64,
    mode: HyphaCycleInputMode,
    release: HyphaContextRelease,
) -> (*mut HyphaCycle, *mut HyphaPublisher, u32) {
    let (publisher, subscriber) = unsafe { create_pubsub(node, &format!("/{}", unique("trig"))) };
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let name = c_text(&unique("triggered"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                period_nanos,
                Some(empty_step),
                ptr::null_mut(),
                release,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let options =
        HyphaCycleBindingOptions { mode: mode as u32, max_samples_per_iteration: 1, optional: 0 };
    let mut index = u32::MAX;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut index) },
        HyphaStatus::Ok
    );
    (cycle, publisher, index)
}

/// Supervise binding `index` of `cycle` with a silence limit.
unsafe fn supervise(cycle: *mut HyphaCycle, node: *mut HyphaNode, index: u32) {
    let bound = unsafe { bind_trigger(cycle, node, index, SILENCE_NANOS / MILLIS) };
    assert_eq!(bound.status, HyphaStatus::Ok, "{}", bound.detail);
    assert_eq!(bound.attached, 1);
}

fn spare_registrations(waitset: *mut HyphaWaitSet) -> usize {
    let record = attach::timer(1_000_000_000, Some(no_interval), ptr::null_mut(), None);
    let mut accepted = Vec::new();
    loop {
        let mut id = 0;
        match unsafe { hypha_waitset_attach(waitset, &raw const record, &raw mut id) } {
            HyphaStatus::Ok => accepted.push(id),
            HyphaStatus::ResourceLimit => break,
            other => panic!("unexpected attach status {other:?}"),
        }
    }
    // The table is measured, not kept full: the `WaitSet` is the node's only one.
    for id in &accepted {
        assert_eq!(unsafe { hypha_waitset_detach(waitset, *id) }, HyphaStatus::Ok);
    }
    accepted.len()
}

struct Fixture {
    node: *mut HyphaNode,
    waitset: *mut HyphaWaitSet,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        // One trigger table per silence limit the cases use.
        let node = node_with_triggers("cyc_trig", &[SILENCE_NANOS / MILLIS, 50, 10_000]);
        let waitset = unsafe { create_waitset(node) };
        Self { node, waitset, _serial: serial }
    }

    fn cycle(&self, period_nanos: u64, mode: HyphaCycleInputMode) -> (*mut HyphaCycle, u32) {
        let (cycle, _, index) =
            unsafe { cycle_with_input(self.waitset, self.node, period_nanos, mode, None) };
        (cycle, index)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe {
            hypha_node_destroy(self.node);
        }
    }
}

#[test]
fn create_accepts_a_zero_period() {
    let fixture = Fixture::new();
    let (cycle, _) = fixture.cycle(0, HyphaCycleInputMode::Queued);
    assert!(!cycle.is_null());
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn commit_refuses_a_zero_period_without_a_trigger() {
    let fixture = Fixture::new();
    let (cycle, _) = fixture.cycle(0, HyphaCycleInputMode::Queued);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidArgument);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn set_trigger_refuses_a_latest_binding() {
    let fixture = Fixture::new();
    let (cycle, index) = fixture.cycle(0, HyphaCycleInputMode::Latest);
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::InvalidArgument);
    assert_eq!(
        unsafe { hypha_cycle_set_trigger(ptr::null_mut(), 0) },
        HyphaStatus::InvalidArgument
    );
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn set_trigger_refuses_an_unknown_binding() {
    let fixture = Fixture::new();
    let (cycle, _) = fixture.cycle(0, HyphaCycleInputMode::Queued);
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, 5) }, HyphaStatus::InvalidArgument);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn set_trigger_refuses_a_committed_cycle() {
    let fixture = Fixture::new();
    let (cycle, index) = fixture.cycle(10_000_000, HyphaCycleInputMode::Queued);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::InvalidState);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn commit_refuses_a_trigger_on_a_cycle_with_a_rate() {
    let fixture = Fixture::new();
    let (cycle, index) = fixture.cycle(10_000_000, HyphaCycleInputMode::Queued);
    unsafe { supervise(cycle, fixture.node, index) };
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidArgument);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn commit_refuses_a_trigger_without_a_silence_limit() {
    let fixture = Fixture::new();
    let (cycle, index) = fixture.cycle(0, HyphaCycleInputMode::Queued);
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidArgument);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn a_trigger_with_a_silence_limit_commits() {
    let fixture = Fixture::new();
    let (cycle, index) = fixture.cycle(0, HyphaCycleInputMode::Queued);
    unsafe { supervise(cycle, fixture.node, index) };
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
}

#[test]
fn destroying_a_triggered_cycle_detaches_both_registrations() {
    let fixture = Fixture::new();
    let expected = spare_registrations(fixture.waitset);

    RELEASES.store(0, Ordering::SeqCst);
    let (cycle, _, index) = unsafe {
        cycle_with_input(
            fixture.waitset,
            fixture.node,
            0,
            HyphaCycleInputMode::Queued,
            Some(count_release),
        )
    };
    unsafe { supervise(cycle, fixture.node, index) };
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
    assert_eq!(spare_registrations(fixture.waitset), expected);
}

#[derive(Debug, Clone, Copy)]
struct Seen {
    /// When the step ran, on the monotonic clock.
    at: std::time::Instant,
    scheduled: i64,
    samples: usize,
    faults: u32,
    /// Triggers consumed without a step, too old when their turn came.
    late_total: u64,
}

#[derive(Default)]
struct Recorder {
    steps: Mutex<Vec<Seen>>,
}

impl Recorder {
    fn steps(&self) -> Vec<Seen> {
        self.steps.lock().map_or_else(|_| Vec::new(), |steps| steps.clone())
    }
}

extern "C" fn record_step(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let recorder = unsafe { &*context.cast::<Recorder>() };
    let mut binding = HyphaCycleBindingView::default();
    let read = unsafe { hypha_cycle_image_binding(image, 0, &raw mut binding) } == HyphaStatus::Ok;
    let (report, supervision) = (binding.health.capture, binding.supervision);
    if !read {
        return HyphaCallbackResult { action: HYPHA_ACTION_FAIL, status: 1 };
    }
    let seen = Seen {
        at: std::time::Instant::now(),
        scheduled: unsafe { (*time).scheduled_nanos },
        samples: report.sample_count,
        faults: supervision.faults,
        late_total: report.stale_total,
    };
    if let Ok(mut steps) = recorder.steps.lock() {
        steps.push(seen);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// A committed triggered cycle on `fixture`'s `WaitSet`, recording into `recorder`.
/// The subscriber holds `depth` samples; the binding takes `arrivals` per step.
fn triggered(
    fixture: &Fixture,
    recorder: &Recorder,
    silence_nanos: i64,
    depth: usize,
    arrivals: usize,
) -> (*mut HyphaCycle, *mut HyphaPublisher) {
    let topic = c_text(&format!("/{}", unique("trig_run")));
    let data_type = c_text("std_msgs/Counter");
    let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
    let definition = c_text("");
    let pub_qos =
        HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() };
    let mut publisher = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_publish_with_qos(
                fixture.node,
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
                fixture.node,
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
    let mut source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(fixture.waitset, subscriber, &raw mut source) },
        HyphaStatus::Ok
    );
    let name = c_text(&unique("triggered_run"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                fixture.waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                0,
                Some(record_step),
                (&raw const *recorder).cast_mut().cast(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    let options = HyphaCycleBindingOptions {
        mode: HyphaCycleInputMode::Queued as u32,
        max_samples_per_iteration: arrivals,
        optional: 0,
    };
    let mut index = u32::MAX;
    assert_eq!(
        unsafe { hypha_cycle_bind_input(cycle, source, &raw const options, &raw mut index) },
        HyphaStatus::Ok
    );
    let bound = unsafe { bind_trigger(cycle, fixture.node, index, silence_nanos / MILLIS) };
    assert_eq!(bound.status, HyphaStatus::Ok, "{}", bound.detail);
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    (cycle, publisher)
}

const MILLIS: i64 = 1_000_000;

#[test]
fn a_triggered_cycle_runs_one_step_per_sample() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let (cycle, publisher) = triggered(&fixture, &recorder, 10_000 * MILLIS, 8, 1);
    for value in 1..=3_u8 {
        unsafe { send_sample(publisher, &[value]) };
    }
    assert!(run_until(fixture.waitset, POLL_BOUND, || recorder.steps().len() >= 3));
    run_waitset_for(fixture.waitset, 20_000_000);
    let steps = recorder.steps();
    assert_eq!(steps.len(), 3, "{steps:?}");
    assert!(steps.iter().all(|step| step.samples == 1), "{steps:?}");
    assert!(steps.windows(2).all(|pair| pair[0].scheduled <= pair[1].scheduled), "{steps:?}");
    assert!(steps.iter().all(|step| step.scheduled <= hypha_clock_steady_now_nanos()));
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
}

#[test]
fn a_triggered_cycle_falls_back_when_the_trigger_is_silent() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let (cycle, publisher) = triggered(&fixture, &recorder, 50 * MILLIS, 8, 1);
    // A host fallback is scheduled on the monotonic clock, not on the wall clock.
    let before = hypha_clock_steady_now_nanos();
    run_waitset_for(fixture.waitset, 130_000_000);
    let steps = recorder.steps();
    assert!(steps.len() >= 2, "{steps:?}");
    assert!(steps.iter().all(|step| step.samples == 0), "{steps:?}");
    assert!(steps.iter().all(|step| step.faults & HYPHA_FAULT_STALE != 0), "{steps:?}");
    assert!(steps[0].scheduled >= before + 50 * MILLIS, "{steps:?}");
    // A late turn runs the fallbacks it missed as one, so the gaps are whole limits.
    assert!(steps.windows(2).all(|pair| {
        let gap = pair[1].scheduled - pair[0].scheduled;
        gap > 0 && gap % (50 * MILLIS) == 0
    }));
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
}

#[test]
fn a_trigger_resets_the_fallback() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let (cycle, publisher) = triggered(&fixture, &recorder, 50 * MILLIS, 8, 1);
    // A trigger whose turn comes more than a limit after its send, as on a loaded host,
    // is consumed as late without a step, and the next step counts it.
    let served = || {
        let steps = recorder.steps();
        let late = steps.last().map_or(0, |step| step.late_total);
        steps.iter().filter(|step| step.samples == 1).count() as u64 + late
    };
    // Ten triggers about 20 ms apart span four silence limits.
    for sent in 1..=10 {
        unsafe { send_sample(publisher, &[1]) };
        assert!(run_until(fixture.waitset, POLL_BOUND, || served() >= sent));
        run_waitset_for(fixture.waitset, 20_000_000);
    }
    let steps = recorder.steps();
    assert_eq!(served(), 10, "{steps:?}");
    // Each trigger restarts the limit, so a fallback runs only a whole limit after the
    // step before it.
    assert!(
        steps.windows(2).all(
            |pair| pair[1].samples == 1 || pair[1].scheduled - pair[0].scheduled >= 50 * MILLIS
        ),
        "{steps:?}"
    );
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
}

#[test]
fn a_burst_of_triggers_runs_one_step_each_in_order() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let (cycle, publisher) = triggered(&fixture, &recorder, 10_000 * MILLIS, 8, 2);
    for value in 1..=5_u8 {
        unsafe { send_sample(publisher, &[value]) };
    }
    assert!(run_until(fixture.waitset, POLL_BOUND, || recorder.steps().len() >= 5));
    run_waitset_for(fixture.waitset, 20_000_000);
    let steps = recorder.steps();
    assert_eq!(steps.len(), 5, "{steps:?}");
    assert!(steps.iter().all(|step| step.samples == 1), "{steps:?}");
    assert!(steps.windows(2).all(|pair| pair[0].scheduled < pair[1].scheduled), "{steps:?}");
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
}

#[test]
fn an_idle_notified_trigger_wakes_its_waitset_on_the_audit_cadence_not_at_silence_over_ten() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    // A 50 ms silence limit: the periodic wake serves silence detection every 25 ms, with
    // the 50 ms audit about 60 turns a second.
    let (cycle, publisher) = triggered(&fixture, &recorder, 50 * MILLIS, 8, 1);
    let before = crate::common::node_metric(fixture.node, "node.sweeps").expect("node.sweeps");
    run_waitset_for(fixture.waitset, 1_000_000_000);
    let after = crate::common::node_metric(fixture.node, "node.sweeps").expect("node.sweeps");
    let turns = after - before;
    let steps = recorder.steps();
    assert!((15..=22).contains(&steps.len()), "fallbacks every 50 ms: {}", steps.len());
    assert!(turns <= 100, "an idle triggered cycle woke its WaitSet {turns} times in 1 s");
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
}

#[test]
fn a_host_triggered_cycle_fallback_runs_at_its_silence_limit_on_steady_time() {
    // One trigger sent at `sent`, then silence: the fallback runs one silence limit after
    // the send, measured on the monotonic clock, within a turn of the deadline.
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let (cycle, publisher) = triggered(&fixture, &recorder, 100 * MILLIS, 8, 1);
    let sent = std::time::Instant::now();
    unsafe { send_sample(publisher, &[1]) };
    assert!(run_until(fixture.waitset, POLL_BOUND, || recorder.steps().len() >= 2));
    let steps = recorder.steps();
    let trigger = steps.iter().find(|step| step.samples == 1).expect("the trigger's step");
    let fallback = steps.iter().find(|step| step.samples == 0).expect("the fallback step");
    let after_send = fallback.at.duration_since(sent);
    assert!(
        (std::time::Duration::from_millis(100)..=std::time::Duration::from_millis(110))
            .contains(&after_send),
        "the fallback ran {after_send:?} after the send; its silence limit is 100 ms"
    );
    assert!(trigger.at <= fallback.at, "{steps:?}");
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
}

/// A host timer that, on its first call, sends one trigger sample and then blocks for
/// 30 ms in the same `WaitSet` turn as the triggered cycle.
struct SlowSender {
    publisher: std::sync::atomic::AtomicPtr<HyphaPublisher>,
    calls: AtomicU32,
}

extern "C" fn send_then_block(_missed: u64, context: *mut c_void) -> HyphaCallbackResult {
    let sender = unsafe { &*context.cast::<SlowSender>() };
    let publisher = sender.publisher.load(Ordering::SeqCst);
    let ready = !publisher.is_null();
    if !ready {
        return HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 };
    }
    let first_call = sender.calls.fetch_add(1, Ordering::SeqCst) == 0;
    if first_call {
        unsafe { send_sample(publisher, &[2]) };
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_fresh_trigger_is_not_late_when_an_earlier_item_of_the_turn_was_slow() {
    // Trigger 2 is sent about 20 ms after trigger 1 ran, by a timer that then blocks for
    // 30 ms in the same turn, so it is not late, whatever the timer cost.
    let fixture = Fixture::new();
    let sender = SlowSender {
        publisher: std::sync::atomic::AtomicPtr::new(ptr::null_mut()),
        calls: AtomicU32::new(0),
    };
    let record = attach::timer(
        20_000_000,
        Some(send_then_block),
        (&raw const sender).cast_mut().cast(),
        None,
    );
    let mut attachment = 0;
    assert_eq!(
        unsafe { hypha_waitset_attach(fixture.waitset, &raw const record, &raw mut attachment) },
        HyphaStatus::Ok
    );
    let recorder = Recorder::default();
    let (cycle, publisher) = triggered(&fixture, &recorder, 10_000 * MILLIS, 8, 1);
    unsafe { send_sample(publisher, &[1]) };
    sender.publisher.store(publisher, Ordering::SeqCst);
    assert!(run_until(fixture.waitset, POLL_BOUND, || recorder.steps().len() >= 2));
    let steps = recorder.steps();
    assert_eq!(steps.len(), 2, "both triggers step: {steps:?}");
    assert!(steps.iter().all(|step| step.late_total == 0), "no trigger is late: {steps:?}");
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    unsafe { hypha_publisher_destroy(publisher) };
}
