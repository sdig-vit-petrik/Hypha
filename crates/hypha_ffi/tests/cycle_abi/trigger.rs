//! Triggered cycles at the C ABI: refusals at registration and commit, and their
//! registrations on the `WaitSet`.
#![allow(
    clippy::indexing_slicing,
    clippy::panic,
    reason = "a test asserts on known values and fails loudly"
)]

use super::*;
use crate::common::attach;
use crate::common::replay::send_stamped;
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
        let node = node_with_triggers("cyc_trig", &[SILENCE_NANOS / MILLIS, 50, 1_000, 10_000]);
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
fn destroying_a_triggered_cycle_detaches_every_registration() {
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
    let second = trigger_input(&fixture, 8);
    let second_index = bind_queued(cycle, second.source, 1);
    add_trigger(&fixture, cycle, second_index, 1_000 * MILLIS);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    // One periodic wake and one listener per trigger.
    assert_eq!(spare_registrations(fixture.waitset), expected - 3);
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    assert_eq!(RELEASES.load(Ordering::SeqCst), 1);
    assert_eq!(spare_registrations(fixture.waitset), expected);
    unsafe { hypha_publisher_destroy(second.publisher) };
}

/// One binding of a step as the test reads it back.
#[derive(Debug, Clone, Copy)]
struct PortSeen {
    samples: usize,
    faults: u32,
    /// Samples refused by age, and late triggers, so far.
    rejected: u64,
}

#[derive(Debug, Clone)]
struct Seen {
    /// When the step ran, on the monotonic clock.
    at: std::time::Instant,
    scheduled: i64,
    samples: usize,
    faults: u32,
    /// Triggers consumed without a step, too old when their turn came.
    late_total: u64,
    /// Every binding, in binding order.
    ports: Vec<PortSeen>,
}

impl Seen {
    /// The binding whose trigger sample this step holds, `None` for a fallback.
    fn fired(&self) -> Option<usize> {
        self.ports.iter().position(|port| port.samples == 1)
    }
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

/// Every binding of `image`, in binding order.
unsafe fn ports_of(image: *const HyphaCycleImage) -> Vec<PortSeen> {
    let mut ports = Vec::new();
    loop {
        let mut binding = HyphaCycleBindingView::default();
        let index = u32::try_from(ports.len()).expect("a few bindings");
        if unsafe { hypha_cycle_image_binding(image, index, &raw mut binding) } != HyphaStatus::Ok {
            return ports;
        }
        let report = binding.health.capture;
        ports.push(PortSeen {
            samples: report.sample_count,
            faults: binding.supervision.faults,
            rejected: report.stale_total,
        });
    }
}

extern "C" fn record_step(
    image: *const HyphaCycleImage,
    time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let recorder = unsafe { &*context.cast::<Recorder>() };
    let ports = unsafe { ports_of(image) };
    let Some(&first) = ports.first() else {
        return HyphaCallbackResult { action: HYPHA_ACTION_FAIL, status: 1 };
    };
    let seen = Seen {
        at: std::time::Instant::now(),
        scheduled: unsafe { (*time).scheduled_nanos },
        samples: first.samples,
        faults: first.faults,
        late_total: first.rejected,
        ports,
    };
    if let Ok(mut steps) = recorder.steps.lock() {
        steps.push(seen);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

/// A fresh topic with its publisher and an input source of a test's `WaitSet`.
struct TriggerInput {
    topic: String,
    publisher: *mut HyphaPublisher,
    source: *mut HyphaInputSource,
}

/// A publisher of `std_msgs/Counter` on a fresh topic, and an input source of
/// `fixture`'s `WaitSet` whose subscriber holds `depth` samples.
fn trigger_input(fixture: &Fixture, depth: usize) -> TriggerInput {
    let name = format!("/{}", unique("trig_run"));
    let topic = c_text(&name);
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
    TriggerInput { topic: name, publisher, source }
}

/// An uncommitted cycle with period 0 on `fixture`'s `WaitSet` that runs `step` on
/// `context`.
fn period_less_cycle(
    fixture: &Fixture,
    step: HyphaCycleStep,
    context: *mut c_void,
) -> *mut HyphaCycle {
    let name = c_text(&unique("triggered_run"));
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                fixture.waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                0,
                step,
                context,
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    cycle
}

/// Bind `source` to `cycle` as a queued input that takes `arrivals` samples per step.
fn bind_queued(cycle: *mut HyphaCycle, source: *mut HyphaInputSource, arrivals: usize) -> u32 {
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
    index
}

/// Make binding `index` of `cycle` a trigger with the table of `silence_nanos`.
fn add_trigger(fixture: &Fixture, cycle: *mut HyphaCycle, index: u32, silence_nanos: i64) {
    let bound = unsafe { bind_trigger(cycle, fixture.node, index, silence_nanos / MILLIS) };
    assert_eq!(bound.status, HyphaStatus::Ok, "{}", bound.detail);
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
}

fn recording(recorder: &Recorder) -> *mut c_void {
    (&raw const *recorder).cast_mut().cast()
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
    let input = trigger_input(fixture, depth);
    let cycle = period_less_cycle(fixture, Some(record_step), recording(recorder));
    let index = bind_queued(cycle, input.source, arrivals);
    add_trigger(fixture, cycle, index, silence_nanos);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    (cycle, input.publisher)
}

/// A committed cycle with one trigger per limit of `silences_nanos`, each on its own
/// topic, recording into `recorder`. Returns the cycle and each trigger's publisher.
fn triggers(
    fixture: &Fixture,
    recorder: &Recorder,
    silences_nanos: &[i64],
) -> (*mut HyphaCycle, Vec<*mut HyphaPublisher>) {
    let cycle = period_less_cycle(fixture, Some(record_step), recording(recorder));
    let publishers = silences_nanos
        .iter()
        .map(|&silence| {
            let input = trigger_input(fixture, 8);
            let index = bind_queued(cycle, input.source, 1);
            add_trigger(fixture, cycle, index, silence);
            input.publisher
        })
        .collect();
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    (cycle, publishers)
}

/// Destroy `cycle` and `publishers`.
fn destroy(cycle: *mut HyphaCycle, publishers: &[*mut HyphaPublisher]) {
    assert_eq!(unsafe { hypha_cycle_destroy(cycle) }, HyphaStatus::Ok);
    for publisher in publishers {
        unsafe { hypha_publisher_destroy(*publisher) };
    }
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
fn an_idle_notified_trigger_wakes_its_waitset_for_its_silence_limit_alone() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    // A 50 ms silence limit: the wake serves silence detection, about 40 turns a second;
    // nothing else wakes the run.
    let (cycle, publisher) = triggered(&fixture, &recorder, 50 * MILLIS, 8, 1);
    let before = crate::common::node_metric(fixture.node, "node.sweeps").expect("node.sweeps");
    run_waitset_for(fixture.waitset, 1_000_000_000);
    let after = crate::common::node_metric(fixture.node, "node.sweeps").expect("node.sweeps");
    let turns = after - before;
    // A loaded host runs fewer fallbacks; their schedule and the wakes per fallback are the
    // claim. Each fallback is due a whole number of silence limits after the previous one.
    let steps = recorder.steps();
    assert!(!steps.is_empty(), "no fallback ran in 1 s");
    for pair in steps.windows(2) {
        let gap = pair[1].scheduled - pair[0].scheduled;
        assert!(gap > 0 && gap % (50 * MILLIS) == 0, "a fallback {gap} ns after the previous one");
    }
    let fallbacks = u64::try_from(steps.len()).expect("steps");
    assert!(
        turns <= 3 * fallbacks + 2,
        "an idle triggered cycle woke its WaitSet {turns} times for {fallbacks} fallbacks"
    );
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

/// The reason the library gave for its last refusal on this thread.
fn last_error() -> String {
    let mut length = 0_usize;
    let sized = unsafe { hypha_last_error(ptr::null_mut(), 0, &raw mut length) };
    assert!(sized == HyphaStatus::Ok || sized == HyphaStatus::TooLarge);
    let mut buffer = vec![0_u8; length];
    assert_eq!(
        unsafe { hypha_last_error(buffer.as_mut_ptr().cast(), buffer.len(), &raw mut length) },
        HyphaStatus::Ok
    );
    buffer.truncate(length);
    String::from_utf8(buffer).expect("the reason is UTF-8")
}

/// The wall clock now, the framework time of a host node.
fn wall_now() -> i64 {
    hypha_clock_now_nanos()
}

#[test]
fn a_second_set_trigger_adds_a_trigger() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let (cycle, publishers) = triggers(&fixture, &recorder, &[1_000 * MILLIS, 10_000 * MILLIS]);
    for publisher in &publishers {
        unsafe { send_sample(*publisher, &[1]) };
    }
    assert!(run_until(fixture.waitset, POLL_BOUND, || recorder.steps().len() >= 2));
    run_waitset_for(fixture.waitset, 20_000_000);
    let fired: Vec<Option<usize>> = recorder.steps().iter().map(Seen::fired).collect();
    assert_eq!(fired, [Some(0), Some(1)], "the first trigger was kept");
    destroy(cycle, &publishers);
}

#[test]
fn the_same_binding_twice_is_one_trigger() {
    let fixture = Fixture::new();
    let spare = spare_registrations(fixture.waitset);
    let recorder = Recorder::default();
    let TriggerInput { publisher, source, .. } = trigger_input(&fixture, 8);
    let cycle = period_less_cycle(&fixture, Some(record_step), recording(&recorder));
    let index = bind_queued(cycle, source, 1);
    add_trigger(&fixture, cycle, index, 10_000 * MILLIS);
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, index) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    // One periodic wake and one listener.
    assert_eq!(spare_registrations(fixture.waitset), spare - 2);
    for value in 1..=2_u8 {
        unsafe { send_sample(publisher, &[value]) };
    }
    assert!(run_until(fixture.waitset, POLL_BOUND, || recorder.steps().len() >= 2));
    run_waitset_for(fixture.waitset, 20_000_000);
    assert_eq!(recorder.steps().len(), 2, "one step per sample");
    destroy(cycle, &[publisher]);
}

#[test]
fn two_ports_on_one_topic_are_refused() {
    let fixture = Fixture::new();
    let TriggerInput { publisher, source, .. } = trigger_input(&fixture, 8);
    let cycle = period_less_cycle(&fixture, Some(empty_step), ptr::null_mut());
    let first = bind_queued(cycle, source, 1);
    let second = bind_queued(cycle, source, 1);
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, first) }, HyphaStatus::Ok);
    // Each sample of the shared history would step twice.
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, second) }, HyphaStatus::InvalidArgument);
    destroy(cycle, &[publisher]);
}

#[test]
fn commit_refuses_any_trigger_without_a_silence_limit_and_names_it() {
    let fixture = Fixture::new();
    let limited = trigger_input(&fixture, 8);
    let unlimited = trigger_input(&fixture, 8);
    let cycle = period_less_cycle(&fixture, Some(empty_step), ptr::null_mut());
    let index = bind_queued(cycle, limited.source, 1);
    add_trigger(&fixture, cycle, index, 1_000 * MILLIS);
    let bare = bind_queued(cycle, unlimited.source, 1);
    assert_eq!(unsafe { hypha_cycle_set_trigger(cycle, bare) }, HyphaStatus::Ok);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidArgument);
    let error = last_error();
    let reason = format!("its trigger input ({}) needs a positive silence limit", unlimited.topic);
    assert!(error.contains(&reason), "{error}");
    destroy(cycle, &[limited.publisher, unlimited.publisher]);
}

#[test]
fn two_triggers_run_one_step_each_in_stamp_order() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let (cycle, publishers) = triggers(&fixture, &recorder, &[1_000 * MILLIS, 10_000 * MILLIS]);
    for port in [0, 1, 0] {
        unsafe { send_sample(publishers[port], &[1]) };
    }
    assert!(run_until(fixture.waitset, POLL_BOUND, || recorder.steps().len() >= 3));
    run_waitset_for(fixture.waitset, 20_000_000);
    let steps = recorder.steps();
    let fired: Vec<Option<usize>> = steps.iter().map(Seen::fired).collect();
    assert_eq!(fired, [Some(0), Some(1), Some(0)], "{steps:?}");
    assert!(
        steps.iter().all(|step| step.ports.iter().map(|port| port.samples).sum::<usize>() == 1)
    );
    assert!(steps.windows(2).all(|pair| pair[0].scheduled <= pair[1].scheduled), "{steps:?}");
    destroy(cycle, &publishers);
}

#[test]
fn a_sample_drained_after_a_later_step_of_another_port_still_steps() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let (cycle, publishers) = triggers(&fixture, &recorder, &[1_000 * MILLIS, 10_000 * MILLIS]);
    let before = wall_now();
    unsafe { send_sample(publishers[0], &[1]) };
    assert!(run_until(fixture.waitset, POLL_BOUND, || !recorder.steps().is_empty()));
    // Port 1's sample was sent before port 0's step, and arrives after it.
    unsafe { send_stamped(publishers[1], 2, before - MILLIS) };
    assert!(run_until(fixture.waitset, POLL_BOUND, || recorder.steps().len() >= 2));
    let steps = recorder.steps();
    let fired: Vec<Option<usize>> = steps.iter().map(Seen::fired).collect();
    assert_eq!(fired, [Some(0), Some(1)], "{steps:?}");
    assert_eq!(steps[1].ports[1].rejected, 0, "not late: {steps:?}");
    assert!(steps[0].scheduled <= steps[1].scheduled, "a step time never goes back");
    destroy(cycle, &publishers);
}

#[test]
fn a_late_trigger_is_counted_on_its_own_port() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let (cycle, publishers) = triggers(&fixture, &recorder, &[1_000 * MILLIS, 10_000 * MILLIS]);
    // Two seconds old: past port 0's limit, within port 1's.
    unsafe { send_stamped(publishers[0], 1, wall_now() - 2_000 * MILLIS) };
    unsafe { send_stamped(publishers[1], 2, wall_now() - 2_000 * MILLIS) };
    assert!(run_until(fixture.waitset, POLL_BOUND, || !recorder.steps().is_empty()));
    run_waitset_for(fixture.waitset, 20_000_000);
    let steps = recorder.steps();
    assert_eq!(steps.len(), 1, "the late sample ran no step: {steps:?}");
    assert_eq!(steps[0].fired(), Some(1));
    let rejected: Vec<u64> = steps[0].ports.iter().map(|port| port.rejected).collect();
    assert_eq!(rejected, [1, 0]);
    destroy(cycle, &publishers);
}

#[test]
fn two_triggers_fall_back_after_the_shortest_silence() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let before = hypha_clock_steady_now_nanos();
    let (cycle, publishers) = triggers(&fixture, &recorder, &[10_000 * MILLIS, 50 * MILLIS]);
    run_waitset_for(fixture.waitset, 130_000_000);
    let steps = recorder.steps();
    assert!(steps.len() >= 2, "{steps:?}");
    assert!(steps[0].scheduled >= before + 50 * MILLIS, "{steps:?}");
    for step in &steps {
        assert_eq!(step.fired(), None, "a fallback holds no sample: {step:?}");
        // Only port 1 has been silent for its own limit.
        assert_eq!(step.ports[0].faults & HYPHA_FAULT_STALE, 0, "{step:?}");
        assert_ne!(step.ports[1].faults & HYPHA_FAULT_STALE, 0, "{step:?}");
    }
    assert!(steps.windows(2).all(|pair| {
        let gap = pair[1].scheduled - pair[0].scheduled;
        gap > 0 && gap % (50 * MILLIS) == 0
    }));
    destroy(cycle, &publishers);
}

#[test]
fn each_trigger_wakes_the_waitset() {
    for port in 0..2 {
        let fixture = Fixture::new();
        let recorder = Recorder::default();
        let (cycle, publishers) = triggers(&fixture, &recorder, &[10_000 * MILLIS, 1_000 * MILLIS]);
        let publisher = publishers[port] as usize;
        let sender = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            unsafe { send_sample(publisher as *mut HyphaPublisher, &[1]) };
            std::time::Instant::now()
        });
        // Nothing but the listener of the port can end this wait before 500 ms.
        run_waitset_for(fixture.waitset, 500_000_000);
        let sent = sender.join().expect("the sender ran");
        let steps = recorder.steps();
        assert_eq!(steps.len(), 1, "port {port}: {steps:?}");
        assert_eq!(steps[0].fired(), Some(port));
        let woke_after = steps[0].at.duration_since(sent);
        assert!(woke_after < std::time::Duration::from_millis(100), "port {port}: {woke_after:?}");
        destroy(cycle, &publishers);
    }
}

#[test]
fn a_wildcard_among_triggers_polls_at_a_tenth_of_the_shortest_limit() {
    let fixture = Fixture::new();
    let recorder = Recorder::default();
    let literal = trigger_input(&fixture, 8);
    let pattern = c_text(&format!("/{}/**", unique("trig_wild")));
    let md5 = c_text("d9d213e8c146065ec958333c617eb1a6");
    let options = HyphaSubscriberOptions {
        depth: 8,
        no_publisher_warning_after_nanos: 0,
        qos: hypha_ffi::HyphaQos::default(),
    };
    let mut wildcard = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_subscribe_typed(
                fixture.node,
                pattern.as_ptr(),
                pattern.as_bytes().len(),
                ptr::null(),
                0,
                md5.as_ptr(),
                md5.as_bytes().len(),
                ptr::null(),
                0,
                &raw const options,
                &raw mut wildcard,
            )
        },
        HyphaStatus::Ok
    );
    let mut wildcard_source = ptr::null_mut();
    assert_eq!(
        unsafe { hypha_input_source_create(fixture.waitset, wildcard, &raw mut wildcard_source) },
        HyphaStatus::Ok
    );
    let cycle = period_less_cycle(&fixture, Some(record_step), recording(&recorder));
    let index = bind_queued(cycle, literal.source, 1);
    add_trigger(&fixture, cycle, index, SILENCE_NANOS);
    let index = bind_queued(cycle, wildcard_source, 1);
    add_trigger(&fixture, cycle, index, 10_000 * MILLIS);
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    let before = crate::common::node_metric(fixture.node, "node.sweeps").expect("node.sweeps");
    run_waitset_for(fixture.waitset, 1_000_000_000);
    let after = crate::common::node_metric(fixture.node, "node.sweeps").expect("node.sweeps");
    let turns = after - before;
    // A 10 ms poll, a tenth of the 100 ms limit: about 100 turns and 10 fallbacks. A poll at
    // a tenth of the wildcard's own limit gives about 10, one every millisecond about 1000.
    assert!((50..=300).contains(&turns), "{turns} turns in 1 s");
    destroy(cycle, &[literal.publisher]);
}

/// The cycle a step tries to destroy, and the status the destroy gave.
struct SelfDestroy {
    cycle: std::sync::atomic::AtomicPtr<HyphaCycle>,
    status: Mutex<Option<HyphaStatus>>,
}

extern "C" fn destroy_own_cycle(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    context: *mut c_void,
) -> HyphaCallbackResult {
    let attempt = unsafe { &*context.cast::<SelfDestroy>() };
    let status = unsafe { hypha_cycle_destroy(attempt.cycle.load(Ordering::SeqCst)) };
    if let Ok(mut held) = attempt.status.lock() {
        *held = Some(status);
    }
    HyphaCallbackResult { action: HYPHA_ACTION_CONTINUE, status: 0 }
}

#[test]
fn a_step_cannot_destroy_its_cycle_and_the_destroy_after_the_run_detaches_every_listener() {
    let fixture = Fixture::new();
    let spare = spare_registrations(fixture.waitset);
    let attempt = SelfDestroy {
        cycle: std::sync::atomic::AtomicPtr::new(ptr::null_mut()),
        status: Mutex::new(None),
    };
    let context = (&raw const attempt).cast_mut().cast();
    let cycle = period_less_cycle(&fixture, Some(destroy_own_cycle), context);
    attempt.cycle.store(cycle, Ordering::SeqCst);
    let inputs: Vec<TriggerInput> = [1_000 * MILLIS, 10_000 * MILLIS]
        .into_iter()
        .map(|silence| {
            let input = trigger_input(&fixture, 8);
            let index = bind_queued(cycle, input.source, 1);
            add_trigger(&fixture, cycle, index, silence);
            input
        })
        .collect();
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::Ok);
    unsafe { send_sample(inputs[1].publisher, &[1]) };
    let refused = || attempt.status.lock().is_ok_and(|status| status.is_some());
    assert!(run_until(fixture.waitset, POLL_BOUND, refused));
    let status = attempt.status.lock().map(|status| *status).expect("status");
    assert_eq!(status, Some(HyphaStatus::InvalidState), "a running WaitSet keeps its cycle");
    assert_eq!(spare_registrations(fixture.waitset), spare - 3, "every listener stays");
    let publishers: Vec<*mut HyphaPublisher> = inputs.iter().map(|input| input.publisher).collect();
    destroy(cycle, &publishers);
    assert_eq!(spare_registrations(fixture.waitset), spare);
}
