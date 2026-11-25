//! A `hypha load` worker: one bus node that publishes and subscribes as its plan says,
//! measures what it sends and receives, and reports it to the coordinator over its
//! stdout. It ends when the coordinator closes its stdin, or on a signal.
//!
//! Each publisher runs on a thread of its own, paced against absolute deadlines on the
//! host monotonic clock. The subscribers are attached to the node's `WaitSet`, which
//! runs on the main thread between the worker's own steps; each callback reads the
//! clock first. The publisher writes the clock into the probe just before its publish
//! call, so a latency is send call to callback entry. That clock is
//! `CLOCK_MONOTONIC`, which every process of a host shares and which never steps.

use std::io::{BufRead as _, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use hypha_node::sdk::clock::steady_now_nanos;
use hypha_node::sdk::{
    BusNode, Delivery, PublisherOptions, RawPublisher, SubscriberCounters, SubscriberOptions,
    WaitSet,
};

use crate::load::histogram::{Histogram, P99};
use crate::load::pacing::Pacer;
use crate::load::probe::{PROBE_DEFINITION, PROBE_TYPE, new_probe, probe_md5, write_header};
use crate::load::protocol::{
    FromWorker, IntervalSample, LINE_PREFIX, PublisherPlan, PublisherResult, Stage, SubscriberPlan,
    SubscriberResult, Times, ToWorker, WorkerPlan, encode,
};
use crate::load::series::larger;

/// The longest a worker waits for the coordinator's next line before it starts.
const START_BOUND: Duration = Duration::from_secs(60);
/// The longest one step of the main loop runs, so that a line from the coordinator is
/// read promptly.
const STEP_NANOS: i64 = 20_000_000;
/// The shortest step.
const MIN_STEP_NANOS: i64 = 1_000_000;
/// The longest one sleep of a publisher, so that an abort is seen promptly.
const MAX_SLEEP_NANOS: i64 = 50_000_000;

/// A line from the coordinator, or why there is none.
enum Control {
    Line(ToWorker),
    Broken(String),
}

/// Reads the coordinator's lines on a thread of its own. The channel ends when stdin
/// closes.
fn read_control() -> Receiver<Control> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let control = match line {
                Ok(line) => serde_json::from_str(&line).map_or_else(
                    |error| {
                        Control::Broken(format!("malformed line from the coordinator: {error}"))
                    },
                    Control::Line,
                ),
                Err(error) => Control::Broken(format!("cannot read stdin: {error}")),
            };
            let broken = matches!(control, Control::Broken(_));
            if sender.send(control).is_err() || broken {
                return;
            }
        }
    });
    receiver
}

/// Writes one protocol line.
fn send(message: &FromWorker) -> Result<(), String> {
    let line = encode(message)?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{LINE_PREFIX}{line}")
        .and_then(|()| out.flush())
        .map_err(|error| format!("cannot write to the coordinator: {error}"))
}

/// The next line, waiting at most [`START_BOUND`].
fn next_line(control: &Receiver<Control>) -> Result<ToWorker, String> {
    match control.recv_timeout(START_BOUND) {
        Ok(Control::Line(line)) => Ok(line),
        Ok(Control::Broken(reason)) => Err(reason),
        Err(RecvTimeoutError::Timeout) => Err("no line from the coordinator".to_owned()),
        Err(RecvTimeoutError::Disconnected) => Err("the coordinator closed stdin".to_owned()),
    }
}

/// Runs the worker `process`; the result is its exit code.
#[must_use]
pub fn run(process: &str) -> u8 {
    match work(process) {
        Ok(()) => 0,
        Err(reason) => {
            // The coordinator may be gone; the reason also goes to stderr.
            let _ = send(&FromWorker::Failed { reason: reason.clone() });
            eprintln!("hypha load worker {process}: {reason}");
            crate::load::EXIT_RUN_FAILED
        }
    }
}

/// What one subscriber counted, shared between its callback and the main loop.
#[derive(Default)]
struct SubscriberStats {
    times: Option<Times>,
    last_sequence: Option<u64>,
    gaps: u64,
    reordered: u64,
    malformed: u64,
    received_total: u64,
    received_window: u64,
    latency: Histogram,
    interval: Histogram,
    worst_interval_p99: Option<u64>,
}

impl SubscriberStats {
    fn on_sample(&mut self, received_at: i64, payload: Option<&[u8]>) {
        let Some((sent, sequence)) = payload.and_then(crate::load::probe::read_header) else {
            self.malformed = self.malformed.saturating_add(1);
            return;
        };
        self.received_total = self.received_total.saturating_add(1);
        match self.last_sequence {
            Some(last) if sequence > last => {
                let skipped = sequence.saturating_sub(last).saturating_sub(1);
                self.gaps = self.gaps.saturating_add(skipped);
                self.last_sequence = Some(sequence);
            }
            Some(_) => self.reordered = self.reordered.saturating_add(1),
            None => self.last_sequence = Some(sequence),
        }
        let (Ok(sent), Some(times)) = (i64::try_from(sent), self.times) else {
            return;
        };
        if !times.in_window(sent) {
            return;
        }
        // The clock is one for every process of the host and never steps back, so a
        // receive before the send is a broken probe, not a latency.
        let Some(latency) = received_at.checked_sub(sent).and_then(|n| u64::try_from(n).ok())
        else {
            self.malformed = self.malformed.saturating_add(1);
            return;
        };
        self.received_window = self.received_window.saturating_add(1);
        self.latency.record(latency);
        self.interval.record(latency);
    }

    /// Ends one sample interval: its 99th percentile, also kept as the worst.
    fn end_interval(&mut self) -> Option<u64> {
        let p99 = self.interval.quantile(P99);
        self.worst_interval_p99 = larger(self.worst_interval_p99, p99);
        self.interval.clear();
        p99
    }
}

fn lock(stats: &Mutex<SubscriberStats>) -> std::sync::MutexGuard<'_, SubscriberStats> {
    stats.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Spins until the clock reads `until`: a slow reader's work, which takes the time
/// whatever the host's load.
fn busy_until(until: i64) {
    while steady_now_nanos() < until {
        std::hint::spin_loop();
    }
}

/// Sleeps until the clock reads `until`, or until `abort` is set.
fn sleep_until(until: i64, abort: &AtomicBool) {
    loop {
        let left = until.saturating_sub(steady_now_nanos());
        if left <= 0 || abort.load(Ordering::Relaxed) {
            return;
        }
        let nanos = u64::try_from(left.min(MAX_SLEEP_NANOS)).unwrap_or(0);
        thread::sleep(Duration::from_nanos(nanos));
    }
}

/// One subscriber of the worker.
struct Subscription {
    topic: String,
    counters: SubscriberCounters,
    stats: Arc<Mutex<SubscriberStats>>,
}

fn work(process: &str) -> Result<(), String> {
    let control = read_control();
    let plan = match next_line(&control)? {
        ToWorker::Plan { plan } if plan.process == process => plan,
        other => return Err(format!("expected the plan of '{process}', got {other:?}")),
    };
    let bus = BusNode::new(&format!("load_{process}"))?;
    let md5 = probe_md5().ok_or("the probe definition has no MD5")?;
    let mut publishers = Vec::with_capacity(plan.publishers.len());
    for publisher in &plan.publishers {
        let options = PublisherOptions::default().bounded(publisher.bytes);
        let opened = bus.publish_raw_configured(
            &publisher.topic,
            PROBE_TYPE,
            &md5,
            PROBE_DEFINITION,
            options,
        )?;
        publishers.push((publisher.clone(), opened));
    }
    send(&FromWorker::Ready { stage: Stage::Publishers })?;

    match next_line(&control)? {
        ToWorker::Subscribe => {}
        other => return Err(format!("expected subscribe, got {other:?}")),
    }
    let mut waitset = WaitSet::new(&bus)?;
    let subscriptions = subscribe(&bus, &mut waitset, &plan.subscribers, &md5)?;
    send(&FromWorker::Ready { stage: Stage::Subscribers })?;

    let times = match next_line(&control)? {
        ToWorker::Start { times } => times,
        other => return Err(format!("expected start, got {other:?}")),
    };
    for subscription in &subscriptions {
        lock(&subscription.stats).times = Some(times);
    }
    let abort = AtomicBool::new(false);
    thread::scope(|scope| {
        let handles: Vec<_> = publishers
            .into_iter()
            .map(|(plan, publisher)| {
                let abort = &abort;
                scope.spawn(move || publish(&plan, publisher, times, abort))
            })
            .collect();
        let monitored = Monitor { bus: &bus, plan: &plan, times, control: &control }.run(
            &mut waitset,
            &subscriptions,
            handles,
        );
        if monitored.is_err() {
            abort.store(true, Ordering::Relaxed);
        }
        monitored
    })
}

fn subscribe(
    bus: &BusNode,
    waitset: &mut WaitSet<'_>,
    plans: &[SubscriberPlan],
    md5: &str,
) -> Result<Vec<Subscription>, String> {
    let mut subscriptions = Vec::with_capacity(plans.len());
    for plan in plans {
        let options = SubscriberOptions::default().keeping(plan.depth);
        let subscriber = bus.subscribe_raw_with_schema(&plan.topic, PROBE_TYPE, md5, options)?;
        let counters = subscriber.counters();
        let stats = Arc::new(Mutex::new(SubscriberStats::default()));
        let shared = Arc::clone(&stats);
        let work_ns = i64::try_from(plan.work_ns).map_err(|_| "work_us is too large")?;
        waitset.attach_subscription(subscriber, move |delivery: &Delivery<'_>| {
            let received_at = steady_now_nanos();
            if work_ns > 0 {
                busy_until(received_at.saturating_add(work_ns));
            }
            lock(&shared).on_sample(received_at, delivery.payload);
            Ok(())
        })?;
        subscriptions.push(Subscription { topic: plan.topic.clone(), counters, stats });
    }
    Ok(subscriptions)
}

/// What one publisher counts while it sends.
struct Sending {
    result: PublisherResult,
    publish_call: Histogram,
    sequence: u64,
}

impl Sending {
    fn new(topic: &str) -> Self {
        Self {
            result: PublisherResult { topic: topic.to_owned(), ..PublisherResult::default() },
            publish_call: Histogram::default(),
            sequence: 0,
        }
    }

    fn send_one(&mut self, publisher: &mut RawPublisher, message: &mut [u8], times: Times) {
        let sent = steady_now_nanos();
        let stamp = u64::try_from(sent).unwrap_or(0);
        let written = write_header(message, stamp, self.sequence);
        let accepted = written && publisher.publish(message);
        let returned = steady_now_nanos();
        self.account(sent, returned, accepted, times);
    }

    /// Counts one publish call that started at `sent` and returned at `returned`.
    fn account(&mut self, sent: i64, returned: i64, accepted: bool, times: Times) {
        let in_window = times.in_window(sent);
        if in_window {
            let call = u64::try_from(returned.saturating_sub(sent)).unwrap_or(0);
            self.publish_call.record(call);
        }
        // The sequence moves with a refused publish too: the framework takes a sequence
        // number for a publish its pool or its send refused (`lend_frame_or_gap`,
        // `send_frame`), so the subscribers count it as missing, and the harness's own
        // gap count must see the same gap. A bounded publisher of an exact-size probe
        // has no other refusal.
        self.sequence = self.sequence.saturating_add(1);
        if !accepted {
            self.result.refused = self.result.refused.saturating_add(1);
            return;
        }
        self.result.sent_total = self.result.sent_total.saturating_add(1);
        if in_window {
            self.result.sent_window = self.result.sent_window.saturating_add(1);
        }
    }
}

/// Publishes `plan` from `times.start` to `times.publish_end`, or until `abort`.
fn publish(
    plan: &PublisherPlan,
    mut publisher: RawPublisher,
    times: Times,
    abort: &AtomicBool,
) -> Result<PublisherResult, String> {
    let mut message = new_probe(plan.bytes)
        .ok_or_else(|| format!("topic '{}': no probe of {} bytes", plan.topic, plan.bytes))?;
    let mut sending = Sending::new(&plan.topic);
    let period = plan.period_ns.map(i64::try_from).transpose().map_err(|_| "period too long")?;
    let burst = match plan.burst {
        Some((count, every)) => Some((count, i64::try_from(every).map_err(|_| "burst too long")?)),
        None => None,
    };
    let mut pacer = Pacer::new(period, burst, times);
    let mut missed: u64 = 0;
    sleep_until(times.start, abort);
    while !abort.load(Ordering::Relaxed) {
        let now = steady_now_nanos();
        if now >= times.publish_end {
            break;
        }
        let due = pacer.due(now);
        missed = missed.saturating_add(due.missed);
        if due.sends == 0 {
            sleep_until(due.wake.min(times.publish_end), abort);
            continue;
        }
        for _ in 0..due.sends {
            sending.send_one(&mut publisher, &mut message, times);
        }
    }
    // A stall to the end of publishing leaves window deadlines no later send counted.
    missed = missed.saturating_add(pacer.unserved());
    sending.result.missed_periods = period.map(|_| missed);
    sending.result.publish_call = sending.publish_call.summary();
    Ok(sending.result)
}

/// The main loop of a started worker.
struct Monitor<'a> {
    bus: &'a BusNode,
    plan: &'a WorkerPlan,
    times: Times,
    control: &'a Receiver<Control>,
}

impl Monitor<'_> {
    fn run(
        &self,
        waitset: &mut WaitSet<'_>,
        subscriptions: &[Subscription],
        handles: Vec<thread::ScopedJoinHandle<'_, Result<PublisherResult, String>>>,
    ) -> Result<(), String> {
        let interval = i64::try_from(self.plan.sample_interval_ns).map_err(|_| "interval")?;
        let mut handles = Some(handles);
        let mut next_sample = self.times.start.saturating_add(interval);
        let mut baseline: Option<Vec<(u64, u64)>> = None;
        let mut slow_callbacks: u64 = 0;
        loop {
            if self.take_stop(handles.is_none())? {
                break;
            }
            if !self.bus.ok() {
                return Err("the node was asked to stop".to_owned());
            }
            let now = steady_now_nanos();
            if baseline.is_none() && now >= self.times.window_start {
                baseline = Some(snapshot(subscriptions));
            }
            if now >= next_sample {
                send(&FromWorker::Sample { subscribers: interval_samples(subscriptions) })?;
                while next_sample <= now {
                    next_sample = next_sample.saturating_add(interval);
                }
            }
            if handles
                .as_ref()
                .is_some_and(|all| all.iter().all(thread::ScopedJoinHandle::is_finished))
            {
                let finished = handles.take().unwrap_or_default();
                let publishers = finished
                    .into_iter()
                    .map(|handle| {
                        handle.join().map_err(|_| "a publisher thread failed".to_owned())?
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                send(&FromWorker::Published { publishers })?;
            }
            let wake = if baseline.is_none() {
                next_sample.min(self.times.window_start)
            } else {
                next_sample
            };
            let step = wake.saturating_sub(now).clamp(MIN_STEP_NANOS, STEP_NANOS);
            let step = Duration::from_nanos(u64::try_from(step).unwrap_or(0));
            if subscriptions.is_empty() {
                thread::sleep(step);
            } else {
                let ran = waitset.run_for(step);
                if let Some(error) = ran.err() {
                    return Err(error);
                }
                slow_callbacks = slow_callbacks.saturating_add(ran.slow_callbacks);
            }
        }
        let start = baseline.unwrap_or_else(|| snapshot(subscriptions));
        let subscribers = subscriptions
            .iter()
            .zip(start)
            .map(|(subscription, (missing, gaps))| result(subscription, missing, gaps))
            .collect();
        send(&FromWorker::Result { subscribers, slow_callbacks })
    }

    /// Reads the coordinator's lines: `true` once it said stop. A stop before every
    /// publisher finished, any other line, or a closed stdin fails the worker.
    fn take_stop(&self, published: bool) -> Result<bool, String> {
        match self.control.try_recv() {
            Ok(Control::Line(ToWorker::Stop)) if published => Ok(true),
            Ok(Control::Line(line)) => Err(format!("unexpected line {line:?}")),
            Ok(Control::Broken(reason)) => Err(reason),
            Err(TryRecvError::Empty) => Ok(false),
            Err(TryRecvError::Disconnected) => Err("the coordinator closed stdin".to_owned()),
        }
    }
}

/// The framework's missing-sequence count and the harness's gap count of each
/// subscriber, read together.
fn snapshot(subscriptions: &[Subscription]) -> Vec<(u64, u64)> {
    subscriptions
        .iter()
        .map(|subscription| {
            let stats = lock(&subscription.stats);
            (subscription.counters.missing_sequences(), stats.gaps)
        })
        .collect()
}

fn interval_samples(subscriptions: &[Subscription]) -> Vec<IntervalSample> {
    subscriptions
        .iter()
        .map(|subscription| IntervalSample {
            topic: subscription.topic.clone(),
            p99_ns: lock(&subscription.stats).end_interval(),
        })
        .collect()
}

fn result(subscription: &Subscription, missing_start: u64, gaps_start: u64) -> SubscriberResult {
    let missing = subscription.counters.missing_sequences();
    let mut stats = lock(&subscription.stats);
    // The interval cut short by the stop counts too.
    let _last = stats.end_interval();
    SubscriberResult {
        topic: subscription.topic.clone(),
        received_window: stats.received_window,
        received_total: stats.received_total,
        missing_sequences: missing.saturating_sub(missing_start),
        harness_gaps: stats.gaps.saturating_sub(gaps_start),
        reordered: stats.reordered,
        malformed: stats.malformed,
        latency: stats.latency.summary(),
        worst_interval_p99_ns: stats.worst_interval_p99,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(sent: u64, sequence: u64) -> Vec<u8> {
        let mut message = new_probe(32).unwrap_or_default();
        assert!(write_header(&mut message, sent, sequence));
        message
    }

    #[test]
    fn a_subscriber_counts_gaps_reorders_and_only_window_latencies() {
        let mut stats = SubscriberStats {
            times: Some(Times { start: 0, window_start: 100, window_end: 200, publish_end: 300 }),
            ..SubscriberStats::default()
        };
        stats.on_sample(60, Some(&probe(50, 0)));
        stats.on_sample(160, Some(&probe(150, 1)));
        stats.on_sample(170, Some(&probe(160, 4)));
        stats.on_sample(180, Some(&probe(170, 3)));
        stats.on_sample(180, Some(&[1, 2, 3]));
        assert_eq!(stats.received_total, 4);
        assert_eq!(stats.received_window, 3);
        assert_eq!(stats.gaps, 2, "sequences 2 and 3 were skipped once");
        assert_eq!(stats.reordered, 1);
        assert_eq!(stats.malformed, 1);
        assert_eq!(stats.latency.count(), 3);
        assert_eq!(stats.latency.max(), Some(10));
        assert_eq!(stats.end_interval(), stats.latency.quantile(P99));
        assert_eq!(stats.end_interval(), None, "an interval without samples has none");
        assert_eq!(stats.worst_interval_p99, stats.latency.quantile(P99));
    }

    #[test]
    fn a_refused_publish_leaves_the_gap_the_framework_counts() {
        let times = Times { start: 0, window_start: 0, window_end: 100, publish_end: 100 };
        let mut sending = Sending::new("/t");
        let mut stats = SubscriberStats { times: Some(times), ..SubscriberStats::default() };
        // Accepted as 0, refused as 1 (the framework took sequence 1 too), accepted as 2.
        for accepted in [true, false, true] {
            let sequence = sending.sequence;
            sending.account(10, 12, accepted, times);
            if accepted {
                stats.on_sample(20, Some(&probe(10, sequence)));
            }
        }
        assert_eq!(sending.result.sent_total, 2);
        assert_eq!(sending.result.refused, 1);
        assert_eq!(sending.publish_call.count(), 3, "a refused call is timed too");
        assert_eq!(stats.gaps, 1, "the harness sees the gap the framework counts");
    }

    #[test]
    fn a_receive_stamped_before_its_send_is_no_latency() {
        let mut stats = SubscriberStats {
            times: Some(Times { start: 0, window_start: 0, window_end: 200, publish_end: 300 }),
            ..SubscriberStats::default()
        };
        stats.on_sample(10, Some(&probe(50, 0)));
        assert_eq!(stats.latency.count(), 0);
        assert_eq!(stats.malformed, 1);
    }
}
