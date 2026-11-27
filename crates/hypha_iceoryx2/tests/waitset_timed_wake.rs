//! A timed deadline ends the native wait at the deadline, not at the next whole millisecond.
//!
//! Linux only: macOS's kernel coalesces timers and defers a wake-up by up to half the wait,
//! so there every wake of a loaded host can come 0.65 ms after a 1.3 ms deadline.
#![cfg(target_os = "linux")]
#![allow(
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    reason = "a failed setup stops the test with its message; Time and Duration operators saturate"
)]

use core::time::Duration;

use hypha_core::port::dispatch::AttachmentId;
use hypha_iceoryx2::Iox2Transport;
use hypha_iceoryx2::waitset::{Dispatcher, EventOutcome, Progression, Trigger};
use hypha_testkit::unique_name;
use hypha_time::{Time, steady_now};

const BUDGET: u32 = 64;

/// Each deadline lies this far after the wake that set it: a whole millisecond and a
/// fraction, so a wait rounded up to whole milliseconds ends 0.7 ms after it.
const OFFSET: hypha_time::Duration = hypha_time::Duration::from_nanos(1_300_000);

/// How late every wake of a wait rounded up to whole milliseconds comes, at least.
const ROUNDED_UP: hypha_time::Duration = hypha_time::Duration::from_nanos(700_000);

/// How late the earliest wake may come.
const EARLIEST: hypha_time::Duration = hypha_time::Duration::from_nanos(300_000);

/// A deadline no run reaches: the dispatcher holds one until an interval moves it.
const FAR: hypha_time::Duration = hypha_time::Duration::from_nanos(10_000_000_000);

/// Records how late each wake for a deadline came. The next deadline lies `OFFSET` after
/// the wake, or, with `moved_by_interval`, `FAR` ahead until the next interval moves it to
/// `OFFSET` after that interval's turn.
struct DeadlineDispatcher {
    deadline: Time,
    moved_by_interval: bool,
    wakes: usize,
    lateness: Vec<hypha_time::Duration>,
}

impl DeadlineDispatcher {
    fn new(wakes: usize, moved_by_interval: bool) -> Self {
        let ahead = if moved_by_interval { FAR } else { OFFSET };
        Self { deadline: steady_now() + ahead, moved_by_interval, wakes, lateness: Vec::new() }
    }

    /// How many wakes came earlier than a wait rounded up to whole milliseconds can.
    fn on_time(&self) -> usize {
        self.lateness.iter().filter(|late| **late < ROUNDED_UP).count()
    }

    fn earliest(&self) -> hypha_time::Duration {
        self.lateness.iter().min().copied().expect("one wake")
    }
}

impl Dispatcher for DeadlineDispatcher {
    fn on_event(&mut self, _attachment: AttachmentId) -> EventOutcome {
        EventOutcome::Drained
    }

    fn on_interval(&mut self, _attachment: AttachmentId, _missed: u64) -> Progression {
        self.deadline = steady_now() + OFFSET;
        Progression::Continue
    }

    fn on_timers(&mut self) -> Progression {
        let now = steady_now();
        if now < self.deadline {
            return Progression::Continue;
        }
        self.lateness.push(now - self.deadline);
        self.deadline = now + if self.moved_by_interval { FAR } else { OFFSET };
        if self.lateness.len() >= self.wakes { Progression::Stop } else { Progression::Continue }
    }

    fn next_deadline(&mut self) -> Option<Time> {
        Some(self.deadline)
    }
}

#[test]
fn a_timed_deadline_ends_the_wait_within_a_fraction_of_a_millisecond() {
    const WAKES: usize = 50;
    let transport = Iox2Transport::with_name(&unique_name("timed_wake")).expect("transport");
    let mut waitset = transport.waitset(&unique_name("timed_wake_ctl")).expect("waitset");
    waitset.attach(None, Trigger::Timed).expect("attach the timer");

    let mut dispatcher = DeadlineDispatcher::new(WAKES, false);
    let _report = waitset.run_for(BUDGET, Duration::from_secs(10), &mut dispatcher);
    assert_eq!(dispatcher.lateness.len(), WAKES, "the run ended before its last deadline");

    // Load only delays a wake, so the earliest wake shows the wait itself. A wait in whole
    // milliseconds, rounded up, wakes every time at least 0.7 ms after its deadline.
    let earliest = dispatcher.earliest();
    assert!(earliest < EARLIEST, "the earliest of {WAKES} wakes came {earliest:?} late");
    // A timer set for the first deadline alone would meet that one only. A tenth of the
    // wakes on time leaves the rest to a loaded host's own wake-up lateness.
    let on_time = dispatcher.on_time();
    assert!(on_time >= WAKES / 10, "{on_time} of {WAKES} wakes came less than 0.7 ms late");
}

#[test]
fn a_deadline_moved_earlier_during_a_wait_is_met() {
    const WAKES: usize = 20;
    let transport = Iox2Transport::with_name(&unique_name("timed_moved")).expect("transport");
    let mut waitset = transport.waitset(&unique_name("timed_moved_ctl")).expect("waitset");
    waitset.attach(None, Trigger::Timed).expect("attach the timer");
    // Each tick ends a wait armed for a deadline far ahead and moves the deadline to 1.3 ms
    // after it: the timer must be set again for the next wait.
    waitset
        .attach(None, Trigger::Periodic(Duration::from_millis(30)))
        .expect("attach the interval");

    let mut dispatcher = DeadlineDispatcher::new(WAKES, true);
    let _report = waitset.run_for(BUDGET, Duration::from_secs(10), &mut dispatcher);
    assert_eq!(dispatcher.lateness.len(), WAKES, "the run ended before its last deadline");

    let earliest = dispatcher.earliest();
    assert!(earliest < EARLIEST, "the earliest of {WAKES} moved deadlines came {earliest:?} late");
    let on_time = dispatcher.on_time();
    assert!(
        on_time >= WAKES / 10,
        "{on_time} of {WAKES} moved deadlines came less than 0.7 ms late"
    );
}
