//! What a node reports about itself: its state.
//!
//! A required input that is wired to a topic counts. The node is `WAITING` while a
//! counted binding that never arrived is not ok, else `DEGRADED` while any counted
//! binding is not ok, else `INPUTS_USABLE`. Bindings of one topic give one input with the
//! worst verdict. Reading the state has no side effect.

use std::sync::PoisonError;

use hypha_core::introspection::{InputBindingRow, InputRole};
use hypha_msgs::hypha_msgs::{CycleState, NodeState, RequiredInput};

use crate::cycle::InputCounts;
use crate::cycle::{
    HYPHA_CYCLE_VERDICT_STALE, HYPHA_CYCLE_VERDICT_USABLE, HYPHA_FAULT_STAGNANT,
    HyphaCycleBindingView,
};
use crate::node::HyphaNode;

/// The `RequiredInput` verdict of one capture verdict.
fn verdict_code(capture_verdict: u32) -> u8 {
    match capture_verdict {
        HYPHA_CYCLE_VERDICT_USABLE => RequiredInput::USABLE,
        HYPHA_CYCLE_VERDICT_STALE => RequiredInput::STALE,
        _ => RequiredInput::NEVER_ARRIVED,
    }
}

/// A larger value is a worse verdict.
fn severity(verdict: u8) -> u8 {
    match verdict {
        RequiredInput::NEVER_ARRIVED => 2,
        RequiredInput::STALE => 1,
        _ => 0,
    }
}

/// One counted binding at its cycle's last capture.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Judged {
    topic: String,
    /// The `RequiredInput` verdict.
    verdict: u8,
    ok: bool,
    /// The age the verdict compared with the limit.
    age_nanos: u64,
    stagnant: bool,
    ignored: bool,
    /// Samples the binding never delivered.
    dropped: u64,
    /// The frames its subscriber refused.
    rejected: u64,
    /// The samples a port of its source could not decode.
    undecodable: u64,
}

/// The state that the judged bindings imply for `node`. A topic's row takes the verdict
/// and age of its worst binding.
fn state_of(node: &str, judged: &[Judged]) -> NodeState {
    let mut inputs: Vec<RequiredInput> = Vec::new();
    let mut waiting = false;
    let mut degraded = false;
    for binding in judged {
        if !binding.ok {
            degraded = true;
            waiting |= binding.verdict == RequiredInput::NEVER_ARRIVED;
        }
        match inputs.iter_mut().find(|input| input.topic == binding.topic) {
            Some(known) => {
                if severity(binding.verdict) > severity(known.verdict) {
                    known.verdict = binding.verdict;
                    known.age_nanos = binding.age_nanos;
                }
                known.stagnant |= binding.stagnant;
                known.ignored &= binding.ignored;
                known.dropped = known.dropped.wrapping_add(binding.dropped);
                known.rejected = known.rejected.max(binding.rejected);
                known.undecodable = known.undecodable.max(binding.undecodable);
            }
            None => {
                inputs.push(RequiredInput {
                    topic: binding.topic.clone(),
                    verdict: binding.verdict,
                    age_nanos: binding.age_nanos,
                    stagnant: binding.stagnant,
                    ignored: binding.ignored,
                    dropped: binding.dropped,
                    rejected: binding.rejected,
                    undecodable: binding.undecodable,
                });
            }
        }
    }
    let state = if waiting {
        NodeState::WAITING
    } else if degraded {
        NodeState::DEGRADED
    } else {
        NodeState::INPUTS_USABLE
    };
    NodeState { node: node.to_owned(), state, inputs, ..NodeState::default() }
}

fn judged_from(topic: &str, view: &HyphaCycleBindingView, counts: InputCounts) -> Judged {
    let faults = view.supervision.faults;
    let health = &view.health;
    Judged {
        topic: topic.to_owned(),
        verdict: verdict_code(health.capture.verdict),
        ok: view.ok != 0,
        age_nanos: view.age_nanos,
        stagnant: faults & HYPHA_FAULT_STAGNANT != 0,
        ignored: view.supervision.ignored != 0,
        dropped: health.overrun_total.wrapping_add(health.work_limit_drops_total),
        rejected: counts.rejected,
        undecodable: counts.undecodable,
    }
}

/// The judgement of every required, wired binding at its cycle's last capture, and one
/// row per committed cycle. A binding whose cycle cannot be read counts as never arrived.
fn judge(node: &HyphaNode, bindings: &[InputBindingRow]) -> (Vec<Judged>, Vec<CycleState>) {
    let waitset = node.default_waitset.lock().unwrap_or_else(PoisonError::into_inner);
    let cycles = waitset.held().and_then(|set| set.cycles().ok());
    let mut rows = Vec::new();
    for cycle in cycles.iter().flat_map(|cycles| cycles.iter()) {
        if !cycle.is_committed() {
            continue;
        }
        let health = *cycle.snapshots.health.lock().unwrap_or_else(PoisonError::into_inner);
        rows.push(CycleState {
            name: cycle.name().to_owned(),
            steps: health.steps,
            missed_periods: health.missed_periods,
            last_step_nanos: health.last_step_nanos,
            max_step_nanos: health.max_step_nanos,
            overruns: health.overruns,
            output_drops: health.output_pool_drops,
        });
    }
    let view_of = |row: &InputBindingRow| -> Option<Judged> {
        let cycle = cycles.as_ref()?.iter().find(|cycle| cycle.name() == row.cycle)?;
        let index = usize::try_from(row.index).ok()?;
        let views = cycle.snapshots.bindings.lock().unwrap_or_else(PoisonError::into_inner);
        let view = views.get(index)?;
        let counts = cycle.snapshots.input_counts.lock().unwrap_or_else(PoisonError::into_inner);
        let source = counts.get(index).copied().unwrap_or_default();
        Some(judged_from(&row.topic, view, source))
    };
    let mut judged = Vec::new();
    for row in bindings {
        let counted = row.role == InputRole::Required && row.subscriber.is_some();
        if !counted {
            continue;
        }
        let binding = view_of(row).unwrap_or_else(|| Judged {
            topic: row.topic.clone(),
            verdict: RequiredInput::NEVER_ARRIVED,
            ..Judged::default()
        });
        judged.push(binding);
    }
    (judged, rows)
}

/// The `~/state` message of `node` now: the input verdicts of its cycles, its callback
/// counts, the log engine's losses and whether the process runs on simulated time.
#[must_use]
pub fn node_state(node: &HyphaNode) -> NodeState {
    let snapshot = node.introspection.snapshot();
    let (judged, cycles) = judge(node, &snapshot.input_bindings);
    let mut state = state_of(&snapshot.identity.node, &judged);
    let (invoked, failures, slow) = node.health.callback_counts();
    state.callbacks_invoked = invoked;
    state.callback_failures = failures;
    state.slow_callbacks = slow;
    state.cycles = cycles;
    let log = hypha_log::health();
    state.log_lines_dropped = log.console_lines_dropped;
    state.log_write_failures = log.write_failures;
    state.sim_time = hypha_time::is_sim_time();
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judged(rows: &[(&str, u8, bool)]) -> Vec<Judged> {
        rows.iter()
            .map(|(topic, verdict, ok)| Judged {
                topic: (*topic).to_owned(),
                verdict: *verdict,
                ok: *ok,
                ..Judged::default()
            })
            .collect()
    }

    #[test]
    fn a_node_without_required_inputs_is_usable() {
        let state = state_of("n", &[]);
        assert_eq!(state.state, NodeState::INPUTS_USABLE);
        assert!(state.inputs.is_empty());
    }

    #[test]
    fn the_worst_binding_of_a_topic_is_its_verdict_and_never_arrived_wins() {
        let state = state_of(
            "n",
            &judged(&[
                ("/a", RequiredInput::USABLE, true),
                ("/b", RequiredInput::STALE, false),
                ("/a", RequiredInput::NEVER_ARRIVED, false),
            ]),
        );
        assert_eq!(state.state, NodeState::WAITING);
        let verdicts: Vec<(&str, u8)> =
            state.inputs.iter().map(|input| (input.topic.as_str(), input.verdict)).collect();
        assert_eq!(verdicts, [("/a", RequiredInput::NEVER_ARRIVED), ("/b", RequiredInput::STALE)]);
    }

    #[test]
    fn a_stale_input_degrades_the_node() {
        let state = state_of(
            "n",
            &judged(&[("/a", RequiredInput::USABLE, true), ("/b", RequiredInput::STALE, false)]),
        );
        assert_eq!(state.state, NodeState::DEGRADED);
    }

    #[test]
    fn the_state_follows_the_ok_bits_not_the_verdicts() {
        let stagnant = state_of("n", &judged(&[("/a", RequiredInput::USABLE, false)]));
        assert_eq!(stagnant.state, NodeState::DEGRADED, "a usable stagnant input is not ok");
        let ignored = state_of("n", &judged(&[("/a", RequiredInput::STALE, true)]));
        assert_eq!(ignored.state, NodeState::INPUTS_USABLE, "an ignored stale input is ok");
        let row = ignored.inputs.first().map(|input| input.verdict);
        assert_eq!(row, Some(RequiredInput::STALE), "the row keeps its verdict");
        let ignored_waiting = state_of("n", &judged(&[("/a", RequiredInput::NEVER_ARRIVED, true)]));
        assert_eq!(ignored_waiting.state, NodeState::INPUTS_USABLE);
    }

    #[test]
    fn every_capture_verdict_has_its_code() {
        assert_eq!(verdict_code(HYPHA_CYCLE_VERDICT_USABLE), RequiredInput::USABLE);
        assert_eq!(verdict_code(HYPHA_CYCLE_VERDICT_STALE), RequiredInput::STALE);
        assert_eq!(verdict_code(99), RequiredInput::NEVER_ARRIVED);
    }

    #[test]
    fn a_topic_row_takes_its_worst_binding_and_sums_what_was_dropped() {
        let usable = Judged {
            topic: "/a".to_owned(),
            verdict: RequiredInput::USABLE,
            ok: true,
            age_nanos: 5,
            stagnant: true,
            ignored: true,
            dropped: 2,
            rejected: 7,
            undecodable: 1,
        };
        let stale = Judged {
            verdict: RequiredInput::STALE,
            ok: false,
            age_nanos: 900,
            stagnant: false,
            ignored: false,
            dropped: 3,
            ..usable.clone()
        };
        let node = state_of("n", &[usable, stale]);
        let rows: Vec<(u8, u64, bool, bool, u64, u64, u64)> = node
            .inputs
            .iter()
            .map(|row| {
                let RequiredInput {
                    verdict, age_nanos, stagnant, ignored, dropped, rejected, ..
                } = row;
                (*verdict, *age_nanos, *stagnant, *ignored, *dropped, *rejected, row.undecodable)
            })
            .collect();
        assert_eq!(rows, [(RequiredInput::STALE, 900, true, false, 5, 7, 1)]);
    }
}
