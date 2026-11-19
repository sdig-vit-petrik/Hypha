//! The changes of one binding's judgement, each reported once, and their log lines.
//!
//!
//! A change of condition, and a change of the ignore flag while faulted, is one event.

use crate::supervision::FAULT_STAGNANT;
use crate::{BindingCapture, InputVerdict};

/// The condition of one binding at a capture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Condition {
    /// No sample yet, and the silence limit (if any) has not passed.
    #[default]
    Waiting,
    /// No sample for the whole silence limit since the first capture.
    NeverArrived,
    Stale,
    Stagnant,
    Usable,
}

impl Condition {
    const fn faulted(self) -> bool {
        matches!(self, Self::NeverArrived | Self::Stale | Self::Stagnant)
    }
}

/// One reported change of a binding's judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgementEvent {
    /// The first usable sample of this binding.
    Usable,
    /// Usable again after a fault.
    UsableAgain,
    /// The newest sample passed the silence limit.
    Stale {
        /// Age of the newest sample, nanoseconds.
        age_nanos: u64,
        /// The silence limit, nanoseconds.
        limit_nanos: u64,
    },
    /// No sample for the whole silence limit since the first capture.
    NeverArrived {
        /// Time since the first capture, nanoseconds.
        age_nanos: u64,
        /// The silence limit, nanoseconds.
        limit_nanos: u64,
    },
    /// The alive value stopped changing.
    Stagnant {
        /// Time the alive value has not changed, nanoseconds.
        unchanged_nanos: u64,
        /// The stagnation limit, nanoseconds.
        limit_nanos: u64,
    },
    /// `setIgnored(true)` while the binding is faulted.
    FaultIgnored,
    /// `setIgnored(false)` while the binding is faulted.
    FaultCountsAgain,
}

/// The last reported judgement of one binding. Starts as waiting, not ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct JudgementTracker {
    condition: Condition,
    ignored: bool,
    was_usable: bool,
}

impl JudgementTracker {
    /// The changes since the last call, in order: the condition first, then the ignore
    /// flag.
    pub(crate) fn observe(
        &mut self,
        capture: &BindingCapture,
        silence_nanos: u64,
    ) -> [Option<JudgementEvent>; 2] {
        let supervision = capture.supervision();
        let stagnant = supervision.is_some_and(|state| state.faults() & FAULT_STAGNANT != 0);
        let ignored = supervision.is_some_and(crate::Supervision::ignored);
        let condition = match capture.verdict {
            InputVerdict::Stale => Condition::Stale,
            InputVerdict::NeverArrived if capture.stale() => Condition::NeverArrived,
            InputVerdict::NeverArrived => Condition::Waiting,
            InputVerdict::Usable if stagnant => Condition::Stagnant,
            InputVerdict::Usable => Condition::Usable,
        };
        let mut events = [None, None];
        if condition != self.condition {
            events[0] = match condition {
                Condition::Waiting => None,
                Condition::Usable if self.was_usable => Some(JudgementEvent::UsableAgain),
                Condition::Usable => Some(JudgementEvent::Usable),
                Condition::Stale => Some(JudgementEvent::Stale {
                    age_nanos: capture.age_nanos,
                    limit_nanos: silence_nanos,
                }),
                Condition::NeverArrived => Some(JudgementEvent::NeverArrived {
                    age_nanos: capture.age_nanos,
                    limit_nanos: silence_nanos,
                }),
                Condition::Stagnant => Some(JudgementEvent::Stagnant {
                    unchanged_nanos: supervision
                        .map_or(0, crate::Supervision::alive_unchanged_nanos),
                    limit_nanos: supervision.map_or(0, |state| state.limits().stagnation_nanos),
                }),
            };
        }
        let ignore_changed_while_faulted = ignored != self.ignored && condition.faulted();
        if ignore_changed_while_faulted {
            events[1] = Some(if ignored {
                JudgementEvent::FaultIgnored
            } else {
                JudgementEvent::FaultCountsAgain
            });
        }
        self.was_usable |= condition == Condition::Usable;
        self.condition = condition;
        self.ignored = ignored;
        events
    }
}

/// Whole milliseconds of `nanos`, rounded down.
const fn millis(nanos: u64) -> u64 {
    nanos / 1_000_000
}

/// The log level and text of `event` for the binding named `subject`. An `advisory`
/// binding's line ends in ` (advisory)`.
#[must_use]
pub fn judgement_line(
    subject: &str,
    event: JudgementEvent,
    advisory: bool,
) -> (log::Level, String) {
    let suffix = if advisory { " (advisory)" } else { "" };
    let (level, text) = match event {
        JudgementEvent::Usable => (log::Level::Info, "is usable".to_owned()),
        JudgementEvent::UsableAgain => (log::Level::Info, "is usable again".to_owned()),
        JudgementEvent::Stale { age_nanos, limit_nanos } => (
            log::Level::Warn,
            format!(
                "is stale: newest sample is {} ms old, silence_ms = {}",
                millis(age_nanos),
                millis(limit_nanos)
            ),
        ),
        JudgementEvent::NeverArrived { age_nanos, limit_nanos } => (
            log::Level::Warn,
            format!(
                "never arrived: no sample for {} ms, silence_ms = {}",
                millis(age_nanos),
                millis(limit_nanos)
            ),
        ),
        JudgementEvent::Stagnant { unchanged_nanos, limit_nanos } => (
            log::Level::Warn,
            format!(
                "is stagnant: alive value unchanged for {} ms, stagnation_ms = {}",
                millis(unchanged_nanos),
                millis(limit_nanos)
            ),
        ),
        JudgementEvent::FaultIgnored => (log::Level::Warn, "fault is ignored".to_owned()),
        JudgementEvent::FaultCountsAgain => (log::Level::Warn, "fault counts again".to_owned()),
    };
    (level, format!("{subject} {text}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_judgement_change_gives_its_line_and_level() {
        let warn = log::Level::Warn;
        let info = log::Level::Info;
        let cases = [
            (JudgementEvent::Usable, info, "in is usable"),
            (
                JudgementEvent::Stale { age_nanos: 250_999_999, limit_nanos: 200_000_000 },
                warn,
                "in is stale: newest sample is 250 ms old, silence_ms = 200",
            ),
            (
                JudgementEvent::NeverArrived { age_nanos: 201_000_000, limit_nanos: 200_000_000 },
                warn,
                "in never arrived: no sample for 201 ms, silence_ms = 200",
            ),
            (
                JudgementEvent::Stagnant { unchanged_nanos: 100_500_000, limit_nanos: 100_000_000 },
                warn,
                "in is stagnant: alive value unchanged for 100 ms, stagnation_ms = 100",
            ),
            (JudgementEvent::UsableAgain, info, "in is usable again"),
            (JudgementEvent::FaultIgnored, warn, "in fault is ignored"),
            (JudgementEvent::FaultCountsAgain, warn, "in fault counts again"),
        ];
        for (event, level, text) in cases {
            assert_eq!(judgement_line("in", event, false), (level, text.to_owned()));
        }
    }

    #[test]
    fn an_advisory_input_line_carries_the_suffix() {
        assert_eq!(
            judgement_line("in", JudgementEvent::UsableAgain, true),
            (log::Level::Info, "in is usable again (advisory)".to_owned())
        );
    }
}
