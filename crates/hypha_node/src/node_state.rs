//! What a node reports about itself: its state.
//!
//! A required input that is wired to a topic counts. The node is `WAITING` while a
//! counted binding that never arrived is not ok, else `DEGRADED` while any counted
//! binding is not ok, else `INPUTS_USABLE`. Bindings of one stream (one topic, type and
//! MD5) give one input with the worst verdict; two schemas of one topic are two inputs.
//! Reading the state has no side effect.

use hypha_core::introspection::{EndpointRow, InputBindingRow, InputRole};
use hypha_msgs::hypha_msgs::{CycleState, NodeState, RequiredInput, TopicSchema};

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
#[derive(Debug, Clone, Default, PartialEq)]
struct Judged {
    topic: String,
    /// The message type of the binding's subscriber.
    data_type: String,
    /// The MD5 of the binding's subscriber.
    md5: String,
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
    /// The other schemas of its topic that are published; `None` before they were checked.
    other_schemas: Option<Vec<TopicSchema>>,
}

/// The state that the judged bindings imply for `node`. A stream's row takes the verdict
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
        let row_of_stream = |input: &&mut RequiredInput| {
            input.topic == binding.topic
                && input.data_type == binding.data_type
                && input.md5 == binding.md5
        };
        match inputs.iter_mut().find(row_of_stream) {
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
                known.other_schemas_checked &= binding.other_schemas.is_some();
                for schema in binding.other_schemas.iter().flatten() {
                    if !known.other_schemas.contains(schema) {
                        known.other_schemas.push(schema.clone());
                    }
                }
            }
            None => {
                inputs.push(RequiredInput {
                    topic: binding.topic.clone(),
                    data_type: binding.data_type.clone(),
                    md5: binding.md5.clone(),
                    verdict: binding.verdict,
                    age_nanos: binding.age_nanos,
                    stagnant: binding.stagnant,
                    ignored: binding.ignored,
                    dropped: binding.dropped,
                    rejected: binding.rejected,
                    undecodable: binding.undecodable,
                    other_schemas_checked: binding.other_schemas.is_some(),
                    other_schemas: binding.other_schemas.clone().unwrap_or_default(),
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

fn judged_from(
    row: &InputBindingRow,
    subscriber: &EndpointRow,
    view: &HyphaCycleBindingView,
    counts: InputCounts,
    other_schemas: Option<Vec<TopicSchema>>,
) -> Judged {
    let faults = view.supervision.faults;
    let health = &view.health;
    Judged {
        topic: row.topic.clone(),
        data_type: subscriber.data_type.clone(),
        md5: subscriber.md5.clone(),
        verdict: verdict_code(health.capture.verdict),
        ok: view.ok != 0,
        age_nanos: view.age_nanos,
        stagnant: faults & HYPHA_FAULT_STAGNANT != 0,
        ignored: view.supervision.ignored != 0,
        dropped: health.overrun_total.wrapping_add(health.work_limit_drops_total),
        rejected: counts.rejected,
        undecodable: counts.undecodable,
        other_schemas,
    }
}

/// The judgement of every required, wired binding at its cycle's last capture, and one
/// row per committed cycle. A binding whose cycle cannot be read counts as never arrived.
fn judge(
    node: &HyphaNode,
    bindings: &[InputBindingRow],
    subscribers: &[EndpointRow],
) -> (Vec<Judged>, Vec<CycleState>) {
    let waitset = crate::locked(&node.default_waitset);
    let cycles = waitset.held().and_then(|set| set.cycles().ok());
    let mut rows = Vec::new();
    for cycle in cycles.iter().flat_map(|cycles| cycles.iter()) {
        if !cycle.is_committed() {
            continue;
        }
        let health = *crate::locked(&cycle.snapshots.health);
        let steps = crate::locked(&cycle.snapshots.step_times).summary();
        rows.push(CycleState {
            name: cycle.name().to_owned(),
            steps: health.steps,
            missed_periods: health.missed_periods,
            last_step_nanos: health.last_step_nanos,
            max_step_nanos: health.max_step_nanos,
            overruns: health.overruns,
            output_drops: health.output_pool_drops,
            step_p50_nanos: steps.p50_nanos,
            step_p90_nanos: steps.p90_nanos,
            step_p99_nanos: steps.p99_nanos,
            recent_step_p50_nanos: steps.recent_p50_nanos,
            recent_step_p90_nanos: steps.recent_p90_nanos,
            recent_step_p99_nanos: steps.recent_p99_nanos,
            recent_step_max_nanos: steps.recent_max_nanos,
        });
    }
    let view_of = |row: &InputBindingRow, subscriber: &EndpointRow| -> Option<Judged> {
        let cycle = cycles.as_ref()?.iter().find(|cycle| cycle.name() == row.cycle)?;
        let index = usize::try_from(row.index).ok()?;
        let views = crate::locked(&cycle.snapshots.bindings);
        let view = views.get(index)?;
        let counts = crate::locked(&cycle.snapshots.input_counts);
        let source = counts.get(index).copied().unwrap_or_default();
        let watches = crate::locked(&cycle.snapshots.watches);
        let others = watches
            .get(index)
            .and_then(Option::as_ref)
            .and_then(crate::topic_streams::Watch::others);
        let other_schemas = others.map(|others| {
            others
                .into_iter()
                .map(|stream| TopicSchema {
                    schema_hash: stream.schema_hash,
                    data_type: stream.data_type,
                    md5: stream.md5,
                })
                .collect()
        });
        Some(judged_from(row, subscriber, view, source, other_schemas))
    };
    let mut judged = Vec::new();
    for row in bindings {
        if row.role != InputRole::Required {
            continue;
        }
        let Some(id) = row.subscriber else { continue };
        let unknown = EndpointRow::new(row.topic.clone(), "");
        let subscriber = subscribers.iter().find(|endpoint| endpoint.id == id).unwrap_or(&unknown);
        let binding = view_of(row, subscriber).unwrap_or_else(|| Judged {
            topic: row.topic.clone(),
            data_type: subscriber.data_type.clone(),
            md5: subscriber.md5.clone(),
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
    let (judged, cycles) = judge(node, &snapshot.input_bindings, &snapshot.subscribers);
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
        assert!(state.inputs.is_empty(), "{:?}", state.inputs);
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
    fn two_schemas_of_one_topic_are_two_rows() {
        let version = |md5: &str, verdict, ok| Judged {
            topic: "/a".to_owned(),
            data_type: "pkg/T".to_owned(),
            md5: md5.to_owned(),
            verdict,
            ok,
            ..Judged::default()
        };
        let state = state_of(
            "n",
            &[
                version("d9d213e8c146065ec958333c617eb1a6", RequiredInput::USABLE, true),
                version("581e4b1bda6a172aa19e1038a38e01eb", RequiredInput::NEVER_ARRIVED, false),
            ],
        );
        let rows: Vec<(&str, u8)> =
            state.inputs.iter().map(|input| (input.md5.as_str(), input.verdict)).collect();
        assert_eq!(
            rows,
            [
                ("d9d213e8c146065ec958333c617eb1a6", RequiredInput::USABLE),
                ("581e4b1bda6a172aa19e1038a38e01eb", RequiredInput::NEVER_ARRIVED),
            ]
        );
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
            data_type: "pkg/T".to_owned(),
            md5: "d9d213e8c146065ec958333c617eb1a6".to_owned(),
            verdict: RequiredInput::USABLE,
            ok: true,
            age_nanos: 5,
            stagnant: true,
            ignored: true,
            dropped: 2,
            rejected: 7,
            undecodable: 1,
            other_schemas: None,
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
