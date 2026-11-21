//! A `WaitSet` wakes for its timers and its notifications alone: nothing polls.
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

use core::time::Duration;
use std::time::Instant;

use hypha_core::port::{QosPolicy, QosRequest, TopicSpec, dispatch::AttachmentId};
use hypha_iceoryx2::Iox2Transport;
use hypha_iceoryx2::waitset::{Dispatcher, EventOutcome, Progression, Trigger};
use hypha_testkit::unique_name;

const BUDGET: u32 = 64;

/// The period of the timer these tests hold: a 5 Hz cycle.
const TIMER_PERIOD: Duration = Duration::from_millis(200);

/// How long each run lasts.
const RUN_LENGTH: Duration = Duration::from_millis(1000);

/// Holds one timer of `TIMER_PERIOD` and counts the turns of the run loop.
struct TimerDispatcher {
    next_due: Instant,
    turns: u32,
    timer_runs: u32,
}

impl TimerDispatcher {
    fn new() -> Self {
        Self { next_due: Instant::now() + TIMER_PERIOD, turns: 0, timer_runs: 0 }
    }
}

impl Dispatcher for TimerDispatcher {
    fn on_event(&mut self, _attachment: AttachmentId) -> EventOutcome {
        EventOutcome::Drained
    }

    fn on_timers(&mut self) -> Progression {
        let due = Instant::now() >= self.next_due;
        if due {
            self.timer_runs += 1;
            self.next_due += TIMER_PERIOD;
        }
        Progression::Continue
    }

    fn on_tick(&mut self) -> Progression {
        self.turns += 1;
        Progression::Continue
    }

    fn next_timed_wait(&mut self) -> Option<Duration> {
        Some(self.next_due.saturating_duration_since(Instant::now()))
    }
}

#[test]
fn a_waitset_with_only_a_timer_wakes_at_the_timer_rate() {
    let transport = Iox2Transport::with_name(&unique_name("cadence_timer")).expect("transport");
    let mut waitset = transport.waitset(&unique_name("cadence_timer_ctl")).expect("waitset");
    waitset.attach(None, Trigger::Timed).expect("attach the timer");

    let mut dispatcher = TimerDispatcher::new();
    let _report = waitset.run_for(BUDGET, RUN_LENGTH, &mut dispatcher);

    // 1000 ms of a 200 ms timer: 5 wakes; a 50 ms poll would add about 15 turns.
    assert!(dispatcher.timer_runs >= 4, "the timer ran {} times", dispatcher.timer_runs);
    assert!(
        dispatcher.turns <= 9,
        "{} turns: the loop woke between timer deadlines",
        dispatcher.turns
    );
}

#[test]
fn an_idle_subscription_adds_no_wake() {
    let transport = Iox2Transport::with_name(&unique_name("cadence_event")).expect("transport");
    let spec = TopicSpec::new(&format!("/{}", unique_name("cadence_event_topic")))
        .with_type("hypha_test/Data")
        .with_qos(QosRequest::Declared(QosPolicy::new("cadence", 8, 16, 8)));
    let subscriber = transport.subscribe(&spec).expect("subscribe");
    let mut waitset = transport.waitset(&unique_name("cadence_event_ctl")).expect("waitset");
    let listener = subscriber.open_listener().expect("open a listener");
    waitset.attach(listener, Trigger::Event).expect("attach the subscription");
    waitset.attach(None, Trigger::Timed).expect("attach the timer");

    let mut dispatcher = TimerDispatcher::new();
    let _report = waitset.run_for(BUDGET, RUN_LENGTH, &mut dispatcher);

    // Nothing is published, so the run wakes for the timer alone, as without the listener.
    assert!(dispatcher.timer_runs >= 4, "the timer ran {} times", dispatcher.timer_runs);
    assert!(dispatcher.turns <= 9, "{} turns: an idle subscription woke the run", dispatcher.turns);
}
