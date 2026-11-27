//! A timed deadline ends the native wait at the deadline, not at the next whole millisecond.
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

use core::time::Duration;
use std::time::Instant;

use hypha_core::port::dispatch::AttachmentId;
use hypha_iceoryx2::Iox2Transport;
use hypha_iceoryx2::waitset::{Dispatcher, EventOutcome, Progression, Trigger};
use hypha_testkit::unique_name;

const BUDGET: u32 = 64;

/// Each deadline lies this far after the wake that set it: a whole millisecond and a
/// fraction, so a wait rounded up to whole milliseconds ends 0.7 ms after it.
const OFFSET: Duration = Duration::from_micros(1_300);

/// How many deadlines the run waits for.
const WAKES: usize = 50;

/// Sets each deadline `OFFSET` after the wake that served the previous one, and records
/// how late each wake came.
struct DeadlineDispatcher {
    deadline: Instant,
    lateness: Vec<Duration>,
}

impl Dispatcher for DeadlineDispatcher {
    fn on_event(&mut self, _attachment: AttachmentId) -> EventOutcome {
        EventOutcome::Drained
    }

    fn on_timers(&mut self) -> Progression {
        let now = Instant::now();
        if now < self.deadline {
            return Progression::Continue;
        }
        self.lateness.push(now - self.deadline);
        self.deadline = now + OFFSET;
        if self.lateness.len() >= WAKES { Progression::Stop } else { Progression::Continue }
    }

    fn next_timed_wait(&mut self) -> Option<Duration> {
        Some(self.deadline.saturating_duration_since(Instant::now()))
    }
}

#[test]
fn a_timed_deadline_ends_the_wait_within_a_fraction_of_a_millisecond() {
    let transport = Iox2Transport::with_name(&unique_name("timed_wake")).expect("transport");
    let mut waitset = transport.waitset(&unique_name("timed_wake_ctl")).expect("waitset");
    waitset.attach(None, Trigger::Timed).expect("attach the timer");

    let mut dispatcher =
        DeadlineDispatcher { deadline: Instant::now() + OFFSET, lateness: Vec::new() };
    let _report = waitset.run_for(BUDGET, Duration::from_secs(10), &mut dispatcher);
    assert_eq!(dispatcher.lateness.len(), WAKES, "the run ended before its last deadline");

    // Load only delays a wake, so the earliest wake shows the wait itself. A wait in whole
    // milliseconds, rounded up, wakes every time at least 0.7 ms after its deadline.
    let earliest = dispatcher.lateness.iter().min().copied().expect("one wake");
    assert!(
        earliest < Duration::from_micros(300),
        "the earliest of {WAKES} wakes came {earliest:?} after its deadline"
    );
}
