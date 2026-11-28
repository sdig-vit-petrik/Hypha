//! The schedule of a data-triggered cycle, as pure rules.
//!
//! A triggered cycle runs one step per sample of its trigger ports, in merge order: by
//! send stamp, then by port, each port in its own order. Each time no event ran for the
//! shortest silence limit of the ports, it runs one fallback step. A port with no limit
//! (zero) counts for no fallback and is never late; a schedule whose ports have none runs
//! only on its triggers. A newest-only port steps on the newest of its due samples and
//! passes the older ones ([`TriggerSchedule::newest_only`]). The node passes the clocks
//! and the pending stamps of each port here as plain values. A port is named by its
//! position in the cycle's trigger list, which is in binding order.

use hypha_cycle::TriggerPorts;

use crate::upstream_hold::{HoldError, Limit};

/// One step of a triggered cycle, at a framework time in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerEvent {
    /// A step on the oldest pending sample of `port`.
    Trigger {
        /// Under simulated time the sample's send stamp; on host time its monotonic time,
        /// raised to the time of the event before it, so step times never go back.
        time: i64,
        /// The port of the sample.
        port: usize,
    },
    /// A step without a trigger sample, after the shortest silence limit without an event.
    Fallback {
        /// The time of the step.
        time: i64,
        /// The ports whose own silence limit has passed at `time`.
        due: TriggerPorts,
    },
    /// The oldest pending sample of newest-only `port` runs no step: a newer sample of the
    /// port is due with it ([`TriggerSchedule::newest_only`]).
    Superseded {
        /// The sample's send stamp: on the wall clock on host time, else framework time.
        stamp: i64,
        /// The port of the sample.
        port: usize,
    },
    /// Host time: the oldest pending sample of `port` runs no step. It is older than the
    /// port's silence limit, or stamped before the port's last step.
    Late {
        /// The sample's send stamp, on the wall clock.
        stamp: i64,
        /// The port of the sample.
        port: usize,
    },
}

/// The clock a triggered cycle runs on.
#[derive(Debug, Clone, Copy)]
pub enum TriggerClock<'a> {
    /// Host time: triggers run at once; a fallback at `F` needs `steady >= F`.
    ///
    /// The schedule runs on `steady`, so a system clock step never moves a fallback; a
    /// trigger stamp's age is counted on `wall`.
    Host {
        /// The wall clock now, in nanoseconds since the Unix epoch. Ages stamps.
        wall: i64,
        /// The monotonic reading now, in nanoseconds. Times the fallbacks.
        steady: i64,
    },
    /// Simulated time: an event at `t` needs `t <= now`, and every upstream reported at
    /// least `t`: `t < limit.at`.
    Sim {
        /// The node's simulated clock, in nanoseconds.
        now: i64,
        /// What the upstream nodes allow.
        limit: &'a Limit,
    },
}

impl TriggerClock<'_> {
    /// The reading the schedule runs on.
    const fn now(self) -> i64 {
        match self {
            Self::Host { steady, .. } => steady,
            Self::Sim { now, .. } => now,
        }
    }

    /// Whether an event at `time` may run: always on host time; under simulated time
    /// once the clock reached it and no upstream can still send a trigger stamped at it.
    fn admits(self, time: i64) -> bool {
        match self {
            Self::Host { .. } => true,
            Self::Sim { now, limit } => time <= now && limit.at.is_none_or(|at| time < at),
        }
    }
}

/// What a turn of a triggered cycle does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerAdmit {
    /// Run the events written to `out`, possibly none.
    Run,
    /// Under simulated time, a pending trigger is stamped before the event that runs
    /// before it. Nothing runs.
    OutOfOrder {
        /// The trigger stamp, in nanoseconds.
        stamp: i64,
        /// The time of the event before it, in nanoseconds.
        before: i64,
    },
}

/// One trigger port of a schedule.
#[derive(Debug)]
struct Port {
    /// The silence limit in nanoseconds; positive, or zero for no limit.
    silence: i64,
    /// The time of the port's last trigger step, before it was raised to keep step
    /// times ascending.
    last_step: Option<i64>,
    /// Whether the port steps on the newest of its due samples only.
    newest: bool,
}

impl Port {
    /// The time at which the port has been silent for its limit: its last step, or `first`
    /// before one, plus the limit. `None` without a limit, or when no clock reaches it.
    fn silent_at(&self, first: i64) -> Option<i64> {
        let timed = self.silence > 0;
        timed.then(|| self.last_step.unwrap_or(first).checked_add(self.silence)).flatten()
    }

    /// Host time: whether its sample of wall age `age`, at schedule time `time`, runs no
    /// step: it is older than the limit, or sent before the sample of the port's last step.
    fn is_late(&self, age: i64, time: i64) -> bool {
        let too_old = self.silence > 0 && age > self.silence;
        let before_its_step = self.last_step.is_some_and(|step| time < step);
        too_old || before_its_step
    }
}

/// The ports, fallback anchor and stall state of one triggered cycle.
#[derive(Debug)]
pub struct TriggerSchedule {
    ports: Vec<Port>,
    /// The shortest silence limit of the ports: the fallback interval. `None` when no port
    /// has a limit: no fallback runs.
    shortest: Option<i64>,
    /// The clock of the first evaluation.
    first: Option<i64>,
    /// The time of the last event run, which no later event precedes.
    ran: Option<i64>,
    /// Whether the last admission stopped at [`MAX_EVENTS_PER_ADMIT`] with events left.
    capped: bool,
}

/// The most events one simulated-time admission writes; a long clock jump runs over
/// several calls.
pub const MAX_EVENTS_PER_ADMIT: usize = 1024;

impl TriggerSchedule {
    /// A schedule for one port per silence limit of `silences`, in nanoseconds; zero is a
    /// port with no limit. `None` unless there are between one and
    /// [`TriggerPorts::CAPACITY`] limits, none negative.
    #[must_use]
    pub fn new(silences: &[i64]) -> Option<Self> {
        let counted = (1..=TriggerPorts::CAPACITY).contains(&silences.len());
        let negative = silences.iter().any(|silence| *silence < 0);
        if !counted || negative {
            return None;
        }
        let shortest = silences.iter().copied().filter(|silence| *silence > 0).min();
        Some(Self {
            ports: silences
                .iter()
                .map(|&silence| Port { silence, last_step: None, newest: false })
                .collect(),
            shortest,
            first: None,
            ran: None,
            capped: false,
        })
    }

    /// This schedule with `ports` newest-only: a sample of such a port that a newer sample
    /// of the port is due with runs no step, and is [`TriggerEvent::Superseded`]; the
    /// newest steps. This is ROS's subscriber queue of one, decided by the reader alone.
    ///
    /// Due means: on host time, pending in this admission, so a backlog that a slow step
    /// left behind steps once, on its newest sample. Under simulated time a step at `T`
    /// sees the samples stamped up to `T`, and a later stamp is its own step, so only a
    /// sample of the same stamp passes the older one; the upstream hold has then brought
    /// every sample stamped `T`, so which sample steps depends on the recording alone, not
    /// on when the node got its turn. A port past [`TriggerPorts::CAPACITY`] is not one.
    #[must_use]
    pub fn newest_only(mut self, ports: TriggerPorts) -> Self {
        for (index, port) in self.ports.iter_mut().enumerate() {
            port.newest = ports.contains(index);
        }
        self
    }

    /// The shortest silence limit of the ports, in nanoseconds: the fallback interval.
    /// `None` when no port has a limit.
    #[must_use]
    pub fn shortest(&self) -> Option<i64> {
        self.shortest
    }

    /// Write to `out` the events that run now, in order: the pending samples of every
    /// port in merge order, `pending` holding one list per port, with the fallbacks due
    /// before each, which a trigger at the fallback time preempts.
    ///
    /// Host time coalesces the fallbacks due before the next trigger into one, and a late
    /// sample is [`TriggerEvent::Late`]. Simulated time runs each fallback once no
    /// upstream can still send a trigger at its time; a trigger stamped before the event
    /// that runs before it is `OutOfOrder`.
    ///
    /// # Errors
    /// [`HoldError::Ports`] when `pending` does not hold one list per port,
    /// [`HoldError::Overflow`] for an unrepresentable time.
    pub fn admit(
        &mut self,
        pending: &[Vec<i64>],
        clock: TriggerClock<'_>,
        out: &mut Vec<TriggerEvent>,
    ) -> Result<TriggerAdmit, HoldError> {
        out.clear();
        self.capped = false;
        if pending.len() != self.ports.len() {
            return Err(HoldError::Ports);
        }
        let first = *self.first.get_or_insert(clock.now());
        if let TriggerClock::Sim { .. } = clock
            && let Some(order) = self.out_of_order(pending)
        {
            return Ok(order);
        }
        let mut merge = Merge::new(pending);
        loop {
            if let TriggerClock::Sim { .. } = clock
                && out.len() >= MAX_EVENTS_PER_ADMIT
            {
                self.capped = true;
                break;
            }
            if let TriggerClock::Host { wall, steady } = clock {
                self.take_late(&mut merge, wall, steady, out)?;
            }
            let anchor = self.anchor(first);
            let fallback = self
                .shortest
                .map(|shortest| anchor.checked_add(shortest).ok_or(HoldError::Overflow))
                .transpose()?;
            let head = merge
                .peek()
                .map(|(stamp, port)| trigger_time(stamp, clock).map(|time| (time, stamp, port)))
                .transpose()?;
            match (head, fallback) {
                (Some((time, stamp, port)), _)
                    if fallback.is_none_or(|fallback| time <= fallback) =>
                {
                    if !clock.admits(time) {
                        break;
                    }
                    merge.advance(port);
                    let event = if self.superseded(&merge, stamp, port, clock)? {
                        TriggerEvent::Superseded { stamp, port }
                    } else {
                        self.run_trigger(time, port)?
                    };
                    out.push(event);
                }
                (_, Some(fallback)) => {
                    let head = head.map(|(time, _, _)| time);
                    let Some(time) = self.fallback_time(fallback, anchor, head, clock)? else {
                        break;
                    };
                    out.push(self.run_fallback(time, first)?);
                }
                (_, None) => break,
            }
        }
        Ok(TriggerAdmit::Run)
    }

    /// Simulated time: the first pending stamp, in merge order, below the event that
    /// runs before it.
    fn out_of_order(&self, pending: &[Vec<i64>]) -> Option<TriggerAdmit> {
        let mut before = self.ran;
        for (stamp, _) in Merge::new(pending) {
            if let Some(before) = before.filter(|before| stamp < *before) {
                return Some(TriggerAdmit::OutOfOrder { stamp, before });
            }
            before = Some(stamp);
        }
        None
    }

    /// Whether the sample stamped `stamp` of `port`, just passed in `merge`, is superseded:
    /// the port is newest-only and its next pending sample is due with it, on host time
    /// any, under simulated time one of the same stamp.
    fn superseded(
        &self,
        merge: &Merge<'_>,
        stamp: i64,
        port: usize,
        clock: TriggerClock<'_>,
    ) -> Result<bool, HoldError> {
        if !self.ports.get(port).ok_or(HoldError::Ports)?.newest {
            return Ok(false);
        }
        Ok(merge.next_of(port).is_some_and(|next| match clock {
            TriggerClock::Host { .. } => true,
            TriggerClock::Sim { .. } => next == stamp,
        }))
    }

    /// Host time: consume the late samples at the head of the merge.
    fn take_late(
        &self,
        merge: &mut Merge<'_>,
        wall: i64,
        steady: i64,
        out: &mut Vec<TriggerEvent>,
    ) -> Result<(), HoldError> {
        while let Some((stamp, port)) = merge.peek() {
            let age = wall.checked_sub(stamp).ok_or(HoldError::Overflow)?;
            let time = host_time(age, steady)?;
            if !self.ports.get(port).ok_or(HoldError::Ports)?.is_late(age, time) {
                break;
            }
            merge.advance(port);
            out.push(TriggerEvent::Late { stamp, port });
        }
        Ok(())
    }

    /// Record the step on the sample of `port` at schedule time `time`.
    fn run_trigger(&mut self, time: i64, port: usize) -> Result<TriggerEvent, HoldError> {
        self.ports.get_mut(port).ok_or(HoldError::Ports)?.last_step = Some(time);
        let time = self.ran.map_or(time, |ran| ran.max(time));
        self.ran = Some(time);
        Ok(TriggerEvent::Trigger { time, port })
    }

    /// Record the fallback step at `time`; its due ports are those silent for their limit.
    fn run_fallback(&mut self, time: i64, first: i64) -> Result<TriggerEvent, HoldError> {
        let silent = |port: &Port| port.silent_at(first).is_some_and(|silent| silent <= time);
        let due = self.ports.iter().enumerate().filter(|(_, port)| silent(port));
        let due = TriggerPorts::of(due.map(|(port, _)| port)).ok_or(HoldError::Ports)?;
        self.ran = Some(time);
        Ok(TriggerEvent::Fallback { time, due })
    }

    /// The time the fallback due at `fallback` runs at, or `None` while it may not run.
    /// Every trigger at or before `fallback` ran first, so `head`, the time of the next
    /// one, is after it.
    ///
    /// Host time runs the fallbacks a late turn missed as one, at the latest due time
    /// before the next trigger. Simulated time runs each at its own time.
    fn fallback_time(
        &self,
        fallback: i64,
        anchor: i64,
        head: Option<i64>,
        clock: TriggerClock<'_>,
    ) -> Result<Option<i64>, HoldError> {
        let TriggerClock::Host { steady, .. } = clock else {
            return Ok(clock.admits(fallback).then_some(fallback));
        };
        let before_head = head.map_or(Some(steady), |head| head.checked_sub(1));
        let bound = steady.min(before_head.ok_or(HoldError::Overflow)?);
        let Some(shortest) = self.shortest.filter(|_| fallback <= bound) else {
            return Ok(None);
        };
        latest_multiple(anchor, shortest, bound).map(Some)
    }

    /// What the next fallback counts from: the last event run, or the first evaluation
    /// when that is later. It never moves back.
    fn anchor(&self, first: i64) -> i64 {
        self.ran.map_or(first, |ran| ran.max(first))
    }

    /// Whether the last admission stopped at [`MAX_EVENTS_PER_ADMIT`] with events due.
    #[must_use]
    pub fn capped(&self) -> bool {
        self.capped
    }

    /// The next fallback deadline on the schedule's clock, `None` before the first
    /// evaluation and for a schedule with no limit.
    #[must_use]
    pub fn next_fallback(&self) -> Option<i64> {
        let first = self.first?;
        self.anchor(first).checked_add(self.shortest?)
    }

    /// The clock up to which this cycle may report, exclusive: the lowest of its first
    /// pending trigger, its next fallback and the upstream limit.
    #[must_use]
    pub fn frontier(&self, pending: &[Vec<i64>], limit: &Limit) -> Option<i64> {
        let head = pending.iter().filter_map(|stamps| stamps.first().copied()).min();
        [head, self.next_fallback(), limit.at].into_iter().flatten().min()
    }
}

/// The k-way merge of the ports' pending stamps: the head with the lowest stamp comes
/// first, a tie goes to the lower port, and each port keeps its own order. It merges at
/// most [`TriggerPorts::CAPACITY`] ports, as many as a schedule has.
struct Merge<'a> {
    pending: &'a [Vec<i64>],
    /// The position of each port's next stamp in its list.
    next: [usize; TriggerPorts::CAPACITY],
}

impl<'a> Merge<'a> {
    fn new(pending: &'a [Vec<i64>]) -> Self {
        Self { pending, next: [0; TriggerPorts::CAPACITY] }
    }

    /// The next stamp with its port.
    fn peek(&self) -> Option<(i64, usize)> {
        self.pending
            .iter()
            .zip(self.next.iter())
            .enumerate()
            .filter_map(|(port, (stamps, &next))| stamps.get(next).map(|&stamp| (stamp, port)))
            .min()
    }

    /// The next stamp of `port`.
    fn next_of(&self, port: usize) -> Option<i64> {
        let next = *self.next.get(port)?;
        self.pending.get(port)?.get(next).copied()
    }

    /// Pass the next stamp of `port`.
    fn advance(&mut self, port: usize) {
        if let Some(next) = self.next.get_mut(port) {
            *next = next.saturating_add(1);
        }
    }
}

impl Iterator for Merge<'_> {
    type Item = (i64, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let head = self.peek()?;
        self.advance(head.1);
        Some(head)
    }
}

/// The schedule time of a pending trigger stamp on `clock`.
fn trigger_time(stamp: i64, clock: TriggerClock<'_>) -> Result<i64, HoldError> {
    let TriggerClock::Host { wall, steady } = clock else {
        return Ok(stamp);
    };
    host_time(wall.checked_sub(stamp).ok_or(HoldError::Overflow)?, steady)
}

/// The monotonic time of a host sample of wall age `age`: the age taken back from the
/// monotonic reading `steady`; a stamp from the future counts as sent now.
fn host_time(age: i64, steady: i64) -> Result<i64, HoldError> {
    steady.checked_sub(age.max(0)).ok_or(HoldError::Overflow)
}

/// The latest `last + k * silence`, `k >= 1`, at or before `bound`.
fn latest_multiple(last: i64, silence: i64, bound: i64) -> Result<i64, HoldError> {
    let span = i128::from(bound).checked_sub(i128::from(last)).ok_or(HoldError::Overflow)?;
    let periods = span.checked_div(i128::from(silence)).ok_or(HoldError::Overflow)?;
    let advance = periods.checked_mul(i128::from(silence)).ok_or(HoldError::Overflow)?;
    let time = i128::from(last).checked_add(advance).ok_or(HoldError::Overflow)?;
    i64::try_from(time).map_err(|_| HoldError::Overflow)
}

#[cfg(test)]
#[expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "a test computes and indexes known values and fails loudly"
)]
mod tests;
