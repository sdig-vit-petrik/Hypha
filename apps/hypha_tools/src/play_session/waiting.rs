//! What a lockstep wait says: the nodes it waits for while it waits, and why it gave up
//! when it ends the replay (#148). The rules are plain on purpose: a node that has not
//! advanced its clock for the timeout ends the wait, whether it went quiet or keeps
//! reporting a clock it never raises.

use hypha_node::sdk::clock_barrier::Participants;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// How long a wait runs before the player names what it waits for.
pub(super) const WAIT_NOTICE: Duration = Duration::from_secs(1);

/// The closing advice of every message that ends a lockstep wait.
pub(super) const LOCKSTEP_ADVICE: &str = "only nodes with a rate or triggered cycle on simulated time take part in the lockstep; a system the lockstep cannot run plays with HYPHA_REPLAY_LOCKSTEP=0 (replay-flow-control.md §4.6)";

/// One wait of the player for the acknowledgements of one `/clock` value.
pub(super) struct AckWait {
    value: u64,
    started: Instant,
    /// Each awaited node's highest clock in this wait and when it reached it.
    advanced: BTreeMap<String, (Option<u64>, Instant)>,
    /// The awaited nodes of the last notice, so a notice repeats only when they change.
    noticed: Option<Vec<String>>,
}

impl AckWait {
    /// A wait for the acknowledgements of `/clock` `value`, from `started`.
    pub(super) fn new(value: u64, started: Instant) -> Self {
        Self { value, started, advanced: BTreeMap::new(), noticed: None }
    }

    /// Records the clocks of the `pending` nodes at `now`: a node whose clock rose advanced.
    pub(super) fn observe(
        &mut self,
        participants: &Participants,
        pending: &[String],
        now: Instant,
    ) {
        for node in pending {
            let clock = participants.get(node).map(|heard| heard.clock_nanos);
            let entry = self.advanced.entry(node.clone()).or_insert((clock, self.started));
            if clock > entry.0 {
                *entry = (clock, now);
            }
        }
    }

    /// The line that names the `pending` nodes, once the wait is older than
    /// [`WAIT_NOTICE`] and each time the set of pending nodes changes after that.
    pub(super) fn notice(
        &mut self,
        participants: &Participants,
        pending: &[String],
        now: Instant,
    ) -> Option<String> {
        let due = now.saturating_duration_since(self.started) >= WAIT_NOTICE;
        if !due || self.noticed.as_deref() == Some(pending) {
            return None;
        }
        self.noticed = Some(pending.to_vec());
        let named: Vec<String> = pending
            .iter()
            .map(|node| match participants.get(node) {
                Some(heard) => format!("'{node}' (last clock {} ns)", heard.clock_nanos),
                None => format!("'{node}' (never reported)"),
            })
            .collect();
        Some(format!(
            "hypha play: waiting for {} to answer /clock {} ns",
            named.join(", "),
            self.value
        ))
    }

    /// The error that ends the wait (the player prefixes it with `hypha play: `), once a `pending` node has not advanced for
    /// `timeout`: every pending node with what it did, and the advice. A wait in which
    /// every pending node keeps reporting a clock it does not raise is named a loop.
    pub(super) fn give_up(
        &self,
        participants: &Participants,
        pending: &[String],
        now: Instant,
        timeout: Duration,
    ) -> Option<String> {
        let stalled = |node: &String| {
            let since = self.advanced.get(node).map_or(self.started, |&(_, at)| at);
            now.saturating_duration_since(since) >= timeout
        };
        if !pending.iter().any(stalled) {
            return None;
        }
        let mut holding = 0_usize;
        let states: Vec<String> = pending
            .iter()
            .map(|node| match participants.get(node) {
                None => format!("'{node}' never reported"),
                Some(heard) if now.saturating_duration_since(heard.at) >= timeout => {
                    format!(
                        "'{node}' stopped reporting: its last clock was {} ns",
                        heard.clock_nanos
                    )
                }
                Some(heard) => {
                    holding = holding.saturating_add(1);
                    format!(
                        "'{node}' reports {} ns but does not advance to {} ns",
                        heard.clock_nanos, self.value
                    )
                }
            })
            .collect();
        let loop_hint = if holding == pending.len() {
            "; each holds for another node's report, as a loop with no rate cycle as its delay element does (replay-flow-control.md §6.4)"
        } else {
            ""
        };
        Some(format!(
            "no progress for {timeout:?} at /clock {} ns: {}{loop_hint}; {LOCKSTEP_ADVICE}",
            self.value,
            states.join("; ")
        ))
    }
}

/// Why a node named by `--nodes` has not reported: whether a node of that name runs.
pub(super) fn missing_reason(node: &str, runs: bool) -> String {
    if runs {
        format!("'{node}' runs but is not on /clock_progress")
    } else {
        format!("no node '{node}' runs in this domain")
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "a missing message is a failed test")]
mod tests {
    use super::{AckWait, Participants, WAIT_NOTICE, missing_reason};
    use std::time::{Duration, Instant};

    const VALUE: u64 = 1_000;
    const TIMEOUT: Duration = Duration::from_secs(5);

    fn names(nodes: &[&str]) -> Vec<String> {
        nodes.iter().map(|node| (*node).to_owned()).collect()
    }

    #[test]
    fn a_wait_is_named_after_the_notice_time_and_again_only_when_its_nodes_change() {
        let start = Instant::now();
        let mut participants = Participants::new(start);
        participants.observe("a", 900, 1, start);
        let mut wait = AckWait::new(VALUE, start);
        let both = names(&["a", "b"]);
        assert_eq!(wait.notice(&participants, &both, start), None, "too early");
        let line = wait.notice(&participants, &both, start + WAIT_NOTICE).expect("a notice");
        assert_eq!(
            line,
            "hypha play: waiting for 'a' (last clock 900 ns), 'b' (never reported) to answer /clock 1000 ns"
        );
        assert_eq!(wait.notice(&participants, &both, start + 2 * WAIT_NOTICE), None, "unchanged");
        let one = names(&["b"]);
        assert!(wait.notice(&participants, &one, start + 3 * WAIT_NOTICE).is_some(), "changed");
    }

    #[test]
    fn a_node_that_keeps_reporting_without_advancing_ends_the_wait_as_a_loop() {
        let start = Instant::now();
        let mut participants = Participants::new(start);
        let mut wait = AckWait::new(VALUE, start);
        let pending = names(&["a", "b"]);
        // Both report a held clock on every resend, and never raise it.
        let mut now = start;
        while now < start + TIMEOUT {
            participants.observe("a", 999, 2, now);
            participants.observe("b", 999, 2, now);
            wait.observe(&participants, &pending, now);
            assert_eq!(wait.give_up(&participants, &pending, now, TIMEOUT), None);
            now += Duration::from_millis(200);
        }
        participants.observe("a", 999, 2, now);
        participants.observe("b", 999, 2, now);
        wait.observe(&participants, &pending, now);
        let message = wait.give_up(&participants, &pending, now, TIMEOUT).expect("the end");
        assert!(
            message.contains("'a' reports 999 ns but does not advance to 1000 ns"),
            "{message}"
        );
        assert!(message.contains("'b' reports 999 ns"), "{message}");
        assert!(message.contains("no rate cycle as its delay element"), "{message}");
        assert!(message.contains("HYPHA_REPLAY_LOCKSTEP=0"), "{message}");
    }

    #[test]
    fn a_node_whose_clock_rises_keeps_the_wait_going() {
        let start = Instant::now();
        let mut participants = Participants::new(start);
        let mut wait = AckWait::new(10_000, start);
        let pending = names(&["a"]);
        let mut now = start;
        for clock in 1..=20_u64 {
            participants.observe("a", clock, 2, now);
            wait.observe(&participants, &pending, now);
            assert_eq!(wait.give_up(&participants, &pending, now, TIMEOUT), None, "advancing");
            now += Duration::from_secs(1);
        }
    }

    #[test]
    fn a_quiet_node_is_named_beside_a_held_one_and_no_loop_is_claimed() {
        let start = Instant::now();
        let mut participants = Participants::new(start);
        let mut wait = AckWait::new(VALUE, start);
        let pending = names(&["slow", "held"]);
        participants.observe("slow", 500, 1, start);
        let now = start + TIMEOUT;
        participants.observe("held", 499, 2, now);
        wait.observe(&participants, &pending, now);
        let message = wait.give_up(&participants, &pending, now, TIMEOUT).expect("the end");
        assert!(
            message.contains("'slow' stopped reporting: its last clock was 500 ns"),
            "{message}"
        );
        assert!(message.contains("'held' reports 499 ns"), "{message}");
        assert!(!message.contains("delay element"), "{message}");
    }

    #[test]
    fn a_node_that_never_reported_is_named_so() {
        let start = Instant::now();
        let participants = Participants::new(start);
        let wait = AckWait::new(VALUE, start);
        let pending = names(&["late"]);
        let message =
            wait.give_up(&participants, &pending, start + TIMEOUT, TIMEOUT).expect("the end");
        assert!(message.contains("'late' never reported"), "{message}");
    }

    #[test]
    fn a_missing_named_node_is_told_apart_from_one_that_runs_without_a_cycle() {
        assert_eq!(missing_reason("plain", true), "'plain' runs but is not on /clock_progress");
        assert_eq!(missing_reason("gone", false), "no node 'gone' runs in this domain");
    }
}
