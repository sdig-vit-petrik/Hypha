//! The acknowledgement rules of a lockstep `/clock`, shared by `hypha play` and the
//! clock driver of a simulator: which nodes take part, which have acknowledged a
//! `/clock` value, which has gone silent, and when the start barrier is met.
//!
//! A value is acknowledged once every participant has reported a clock at or beyond
//! it for a `/clock` sample sent at or after its send floor; host monotonic send time
//! tells a rewind's samples from the old ones.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Longest start wait for the nodes a driver or player names.
pub const NAMED_START_TIMEOUT: Duration = Duration::from_secs(30);
/// How often an unacknowledged `/clock` value is sent again.
pub const CLOCK_RESEND_INTERVAL: Duration = Duration::from_millis(200);
/// The samples kept of `/clock_progress` for a node that is behind.
pub const PROGRESS_DEPTH: usize = 8;

/// Bytes of a `rosgraph_msgs/Clock` payload: a ROS time, two `u32`.
pub const CLOCK_PAYLOAD_BYTES: usize = 8;
/// The largest `hypha_msgs/ReplayProgress` payload: name length prefix, the longest
/// node name and two `u64`.
pub const PROGRESS_MAX_PAYLOAD_BYTES: usize = 4 + hypha_core::port::MAX_NODE_NAME_LENGTH + 16;

/// What the player has heard from one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heard {
    /// The clock value of the node's newest report.
    pub clock_nanos: u64,
    /// The send time of the `/clock` sample of the node's newest report.
    pub sent_nanos: u64,
    /// When that report arrived.
    pub at: Instant,
}

/// The nodes that take part in a replay, and what each has reported.
#[derive(Debug, Clone)]
pub struct Participants {
    heard: BTreeMap<String, Heard>,
    last_join: Instant,
}

impl Participants {
    /// No participant yet. `now` is the start of the start barrier.
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self { heard: BTreeMap::new(), last_join: now }
    }

    /// Records that `node` reported `clock_nanos` for the `/clock` sample sent at
    /// `sent_nanos`. Returns true when the node has not been heard from before.
    pub fn observe(&mut self, node: &str, clock_nanos: u64, sent_nanos: u64, now: Instant) -> bool {
        let heard = Heard { clock_nanos, sent_nanos, at: now };
        let is_new = self.heard.insert(node.to_owned(), heard).is_none();
        if is_new {
            self.last_join = now;
        }
        is_new
    }

    /// What `node` last reported, or `None` when it has never reported.
    #[must_use]
    pub fn get(&self, node: &str) -> Option<&Heard> {
        self.heard.get(node)
    }

    /// The names of every participant, in order.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.heard.keys().cloned().collect()
    }

    /// When the newest participant first reported.
    #[must_use]
    pub fn last_join(&self) -> Instant {
        self.last_join
    }

    /// The number of participants.
    #[must_use]
    pub fn len(&self) -> usize {
        self.heard.len()
    }

    /// Whether no node has reported.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.heard.is_empty()
    }
}

/// The nodes of `required` that have not acknowledged the `/clock` sample for `value`, in
/// order: a node's newest report must be sent at or after `floor` with a clock at or
/// beyond `value`.
#[must_use]
pub fn unacknowledged(
    participants: &Participants,
    required: &[String],
    floor: u64,
    value: u64,
) -> Vec<String> {
    required
        .iter()
        .filter(|node| {
            participants
                .get(node)
                .is_none_or(|heard| heard.sent_nanos < floor || heard.clock_nanos < value)
        })
        .cloned()
        .collect()
}

/// A participant that has reported nothing for the timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Silent {
    /// The node's name.
    pub node: String,
    /// The clock value of its last report, or `None` when it never reported.
    pub last_clock_nanos: Option<u64>,
}

/// The node of `pending` quiet the longest past `timeout`, counted from `since` or its
/// last report, whichever is later. A tie goes to the earlier node of `pending`.
#[must_use]
pub fn first_silent(
    participants: &Participants,
    pending: &[String],
    since: Instant,
    now: Instant,
    timeout: Duration,
) -> Option<Silent> {
    pending
        .iter()
        .filter_map(|node| {
            let heard = participants.get(node);
            let quiet_since = heard.map_or(since, |heard| heard.at.max(since));
            (now.saturating_duration_since(quiet_since) >= timeout).then_some((
                quiet_since,
                node,
                heard,
            ))
        })
        // `min_by_key` returns the first of equal minima, so a tie keeps the order of `pending`.
        .min_by_key(|(quiet_since, _, _)| *quiet_since)
        .map(|(_, node, heard)| Silent {
            node: node.clone(),
            last_clock_nanos: heard.map(|heard| heard.clock_nanos),
        })
}

/// Where the start barrier stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartState {
    /// Keep waiting.
    Waiting,
    /// The barrier is met. The nodes that reported are the participants.
    Ready,
    /// No node reported within the settle time. The replay paces by `--rate` alone.
    NoneSeen,
    /// Some node named by `--nodes` did not report within [`NAMED_START_TIMEOUT`].
    Missing(Vec<String>),
}

/// Where the start barrier stands at `now`.
///
///
/// With `named` empty the barrier is met once a node has reported and none has been
/// new for `settle`; with `named` set, once every named node has reported. A
/// connected publisher that has not reported delays the start by at most `settle`.
#[must_use]
pub fn start_state(
    participants: &Participants,
    named: &[String],
    started: Instant,
    now: Instant,
    settle: Duration,
    publishers: usize,
) -> StartState {
    let ready = if !named.is_empty() {
        let missing: Vec<String> =
            named.iter().filter(|node| participants.get(node).is_none()).cloned().collect();
        if missing.is_empty() {
            true
        } else if now.saturating_duration_since(started) >= NAMED_START_TIMEOUT {
            return StartState::Missing(missing);
        } else {
            false
        }
    } else if participants.is_empty() {
        return if now.saturating_duration_since(started) >= settle {
            StartState::NoneSeen
        } else {
            StartState::Waiting
        };
    } else {
        now.saturating_duration_since(participants.last_join()) >= settle
    };
    let unheard_publisher = publishers > participants.len();
    if ready && !(unheard_publisher && now.saturating_duration_since(started) < settle) {
        StartState::Ready
    } else {
        StartState::Waiting
    }
}

/// The settings of a clock driver: a simulator that publishes `/clock` itself.
#[derive(Debug, Clone, PartialEq)]
pub struct DriverOptions {
    /// The first `/clock` value, in nanoseconds.
    pub start_nanos: i64,
    /// The clock step, in nanoseconds. Positive.
    pub period_nanos: i64,
    /// A required participant silent this long fails the run. Positive.
    pub timeout: Duration,
    /// Simulated seconds per host second. `None` steps as fast as the participants allow.
    pub real_time_factor: Option<f64>,
    /// Node names that must acknowledge the start value before the first step.
    pub expected: Vec<String>,
}

/// What a clock driver does on one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverAction {
    /// Run the step at `deadline`, publish `/clock = deadline`, then call
    /// [`DriverSchedule::sent`].
    Step {
        /// The new clock value, in nanoseconds.
        deadline: i64,
    },
    /// Publish `/clock = value` with no step, then call [`DriverSchedule::sent`]: the
    /// start value, or an awaited value again.
    Publish {
        /// The clock value awaited, in nanoseconds.
        value: i64,
    },
    /// Nothing to do this turn.
    Wait,
    /// Fail the run: the next clock value is not representable.
    Exhausted,
    /// Fail the run with `Timeout`.
    Fail {
        /// The silent participant, or the expected names that never reported.
        silent: Vec<String>,
        /// The last clock the silent participant reported, if any.
        last_clock_nanos: Option<u64>,
    },
}

/// The `/clock` value a driver has sent and awaits.
#[derive(Debug, Clone, Copy)]
struct Awaited {
    value: i64,
    floor: u64,
    sent_at: Instant,
    last_send: Instant,
}

/// The lockstep schedule of a clock driver: `/clock` advances only once every required
/// participant has acknowledged the previous value. The start value is the start
/// barrier, as `hypha play` runs it: it goes out with no step, and the first step, at
/// start + period, waits until every expected participant has acknowledged it. A step's
/// outputs therefore reach every expected participant.
#[derive(Debug, Clone)]
pub struct DriverSchedule {
    options: DriverOptions,
    participants: Participants,
    awaited: Option<Awaited>,
    /// When the start value first went out: the start barrier counts from it.
    started: Option<Instant>,
    /// When the start value was acknowledged: the pace counts from it.
    pace_anchor: Option<Instant>,
}

impl DriverSchedule {
    /// A schedule that has sent nothing yet.
    ///
    /// # Errors
    /// A message naming the setting when the period, timeout or factor is not positive,
    /// or no non-empty participant name is expected.
    pub fn new(options: DriverOptions, now: Instant) -> Result<Self, String> {
        if options.start_nanos < 0 {
            return Err("the start clock value must not be negative".to_owned());
        }
        if options.period_nanos <= 0 {
            return Err("the clock period must be positive".to_owned());
        }
        if options.timeout.is_zero() {
            return Err("the participant timeout must be positive".to_owned());
        }
        if options.real_time_factor.is_some_and(|factor| !(factor.is_finite() && factor > 0.0)) {
            return Err("the real-time factor must be positive and finite".to_owned());
        }
        if options.expected.is_empty() {
            return Err("the clock driver expects no participant: name at least one, or it \
                        steps before any node joins"
                .to_owned());
        }
        if options.expected.iter().any(String::is_empty) {
            return Err("an expected participant name is empty".to_owned());
        }
        Ok(Self {
            options,
            participants: Participants::new(now),
            awaited: None,
            started: None,
            pace_anchor: None,
        })
    }

    /// Record a `/clock_progress` report of `node`.
    pub fn observe(&mut self, node: &str, clock_nanos: u64, sent_nanos: u64, now: Instant) {
        self.participants.observe(node, clock_nanos, sent_nanos, now);
    }

    /// Record that `/clock = value` went out, with the host monotonic time `floor_nanos`
    /// taken just before it. A resend keeps the first floor.
    pub fn sent(&mut self, value: i64, floor_nanos: u64, now: Instant) {
        self.started.get_or_insert(now);
        match self.awaited.as_mut() {
            Some(awaited) if awaited.value == value => awaited.last_send = now,
            _ => {
                self.awaited =
                    Some(Awaited { value, floor: floor_nanos, sent_at: now, last_send: now });
            }
        }
    }

    /// What to do at `now`.
    #[must_use]
    pub fn next(&mut self, now: Instant) -> DriverAction {
        let Some(awaited) = self.awaited else {
            return DriverAction::Publish { value: self.options.start_nanos };
        };
        let mut required = self.participants.names();
        for name in &self.options.expected {
            if !required.contains(name) {
                required.push(name.clone());
            }
        }
        required.sort();
        let Ok(value) = u64::try_from(awaited.value) else {
            return DriverAction::Exhausted;
        };
        let pending = unacknowledged(&self.participants, &required, awaited.floor, value);
        if pending.is_empty() {
            if awaited.value == self.options.start_nanos && self.pace_anchor.is_none() {
                let answered = required.iter().filter_map(|node| self.participants.get(node));
                let last = answered.map(|heard| heard.at).max();
                self.pace_anchor = Some(last.map_or(awaited.sent_at, |at| at.max(awaited.sent_at)));
            }
            return self.step_after(awaited.value, now);
        }
        let at_start = awaited.value == self.options.start_nanos;
        let started = self.started.unwrap_or(awaited.sent_at);
        // At the start an expected name that never reported has the longer start timeout.
        let (unheard, heard): (Vec<String>, Vec<String>) =
            pending.into_iter().partition(|node| at_start && self.participants.get(node).is_none());
        if let Some(silent) =
            first_silent(&self.participants, &heard, awaited.sent_at, now, self.options.timeout)
        {
            return DriverAction::Fail {
                silent: vec![silent.node],
                last_clock_nanos: silent.last_clock_nanos,
            };
        }
        if !unheard.is_empty() && now.saturating_duration_since(started) >= NAMED_START_TIMEOUT {
            return DriverAction::Fail { silent: unheard, last_clock_nanos: None };
        }
        if now.saturating_duration_since(awaited.last_send) >= CLOCK_RESEND_INTERVAL {
            return DriverAction::Publish { value: awaited.value };
        }
        DriverAction::Wait
    }

    /// The step after `value` once it is acknowledged, if the pace allows it at `now`.
    fn step_after(&self, value: i64, now: Instant) -> DriverAction {
        let Some(deadline) = value.checked_add(self.options.period_nanos) else {
            return DriverAction::Exhausted;
        };
        let (Some(factor), Some(anchor)) = (self.options.real_time_factor, self.pace_anchor) else {
            return DriverAction::Step { deadline };
        };
        let simulated = deadline.saturating_sub(self.options.start_nanos);
        #[allow(clippy::cast_precision_loss, reason = "a pace needs no nanosecond precision")]
        let host_seconds = simulated as f64 / 1e9 / factor;
        let due = Duration::try_from_secs_f64(host_seconds)
            .ok()
            .and_then(|wait| anchor.checked_add(wait));
        match due {
            Some(due) if now < due => DriverAction::Wait,
            Some(_) => DriverAction::Step { deadline },
            None => DriverAction::Exhausted,
        }
    }
}

/// The payload of `rosgraph_msgs/Clock` for `nanos`: seconds, then nanoseconds, each a
/// little-endian `u32`. `None` when the seconds do not fit or `nanos` is negative.
#[must_use]
pub fn clock_payload(nanos: i64) -> Option<[u8; CLOCK_PAYLOAD_BYTES]> {
    const NANOS_PER_SECOND: i64 = 1_000_000_000;
    let secs = u32::try_from(nanos.div_euclid(NANOS_PER_SECOND)).ok()?.to_le_bytes();
    let subsec = u32::try_from(nanos.rem_euclid(NANOS_PER_SECOND)).ok()?.to_le_bytes();
    let mut payload = [0_u8; CLOCK_PAYLOAD_BYTES];
    let (head, tail) = payload.split_at_mut(4);
    head.copy_from_slice(&secs);
    tail.copy_from_slice(&subsec);
    Some(payload)
}

#[cfg(test)]
mod tests;
