//! Capture admission and freshness on host or replay time.

use super::{CaptureClock, CycleHistory, HistorySample};

impl CaptureClock {
    /// The clock of a trigger port: a replay step admits its trigger samples through its
    /// time; host time admits every retained sample on any port.
    pub(crate) const fn for_trigger(self) -> Self {
        match self {
            Self::Replay { cut_nanos } => Self::ReplayThrough { cut_nanos },
            other => other,
        }
    }

    /// Whether this clock cuts by data time.
    pub(super) const fn is_replay(self) -> bool {
        matches!(self, Self::Replay { .. } | Self::ReplayThrough { .. })
    }

    /// Whether this clock admits a sample with framework send time `sent_nanos`.
    const fn admits_sent(self, sent_nanos: u64) -> bool {
        match self {
            Self::Host { .. } => true,
            Self::Replay { cut_nanos } => sent_nanos < cut_nanos,
            Self::ReplayThrough { cut_nanos } => sent_nanos <= cut_nanos,
        }
    }

    /// An admitted prefix can be indexed directly.
    pub(super) fn admitted_end(self, history: &CycleHistory) -> Option<usize> {
        match self {
            Self::Host { .. } => Some(history.samples().len()),
            Self::Replay { .. } | Self::ReplayThrough { .. } => {
                history.replay_end(|sent_nanos| self.admits_sent(sent_nanos))
            }
        }
    }

    /// Whether this capture may select `sample` at all.
    pub(super) const fn admits(self, sample: &HistorySample) -> bool {
        self.admits_sent(sample.framework_sent_nanos)
    }

    /// The time of this clock: host monotonic time, or the replay cut.
    pub(super) const fn nanos(self) -> u64 {
        match self {
            Self::Host { steady_nanos } => steady_nanos,
            Self::Replay { cut_nanos } | Self::ReplayThrough { cut_nanos } => cut_nanos,
        }
    }

    /// The send time of `sample` on this clock.
    pub(super) const fn sent(self, sample: &HistorySample) -> u64 {
        match self {
            Self::Host { .. } => sample.transport_sent_nanos,
            Self::Replay { .. } | Self::ReplayThrough { .. } => sample.framework_sent_nanos,
        }
    }

    /// Whether `sample` passes a silence limit of `silence_nanos` on this clock.
    pub(super) fn usable(self, silence_nanos: u64, sample: &HistorySample) -> bool {
        value_usable_within(silence_nanos, self.sent(sample), self.nanos())
    }
}

/// Return whether a value sent at `transport_sent_nanos` is usable at
/// `now_nanos` under `silence_nanos`. A zero silence limit keeps every earlier value usable.
pub(super) fn value_usable_within(
    silence_nanos: u64,
    transport_sent_nanos: u64,
    now_nanos: u64,
) -> bool {
    let sent_in_the_future = transport_sent_nanos > now_nanos;
    if sent_in_the_future {
        return false;
    }
    let age = now_nanos.saturating_sub(transport_sent_nanos);
    let has_limit = silence_nanos != 0;
    !has_limit || age <= silence_nanos
}

/// The accepted count that `clock` admits, less the samples a reader rejected. A replay
/// clock admits the samples stamped before its cut, or through it.
pub(super) fn accepted_on(history: &CycleHistory, clock: CaptureClock) -> u64 {
    let stats = history.stats();
    let accepted = stats.accepted.wrapping_sub(stats.rejected);
    if !clock.is_replay() {
        return accepted;
    }
    let retained_after_cut = clock.admitted_end(history).map_or_else(
        || history.samples().iter().filter(|sample| !clock.admits(sample)).count(),
        |end| history.samples().len().saturating_sub(end),
    );
    accepted.wrapping_sub(u64::try_from(retained_after_cut).unwrap_or(u64::MAX))
}
