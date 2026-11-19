//! The rules of a restart after simulated time went back: the run ends with
//! `ClockRewound`, the process exits 75, and `hypha_launch` restarts it with
//! [`REJOIN_ENV`] set; the new process reads no `/clock` sample of the old timeline.

use std::sync::atomic::{AtomicBool, Ordering};

/// Marks a restarted process: a steady time before which every `/clock` sample belongs
/// to the old timeline.
pub const REJOIN_ENV: &str = "HYPHA_REJOIN_AFTER_NANOS";

/// The rejoin of one restarted process. A process keeps one, for its whole life.
#[derive(Debug, Default)]
pub struct RestartState {
    /// Set once a node of the process has claimed the rejoin, so a later node does not wait.
    rejoined: AtomicBool,
}

impl RestartState {
    /// No rejoin claimed.
    #[must_use]
    pub const fn new() -> Self {
        Self { rejoined: AtomicBool::new(false) }
    }

    /// Claim the rejoin of this process. Only the first node that asks gets `true`.
    pub fn claim_rejoin(&self) -> bool {
        !self.rejoined.swap(true, Ordering::SeqCst)
    }
}

/// The send time that a [`REJOIN_ENV`] marker names, or `None` when it is not a number.
#[must_use]
pub fn parse_rejoin_marker(text: &str) -> Option<u64> {
    text.parse::<u64>().ok()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_node_claims_the_rejoin() {
        let state = RestartState::new();
        assert!(state.claim_rejoin());
        assert!(!state.claim_rejoin());
    }

    #[test]
    fn a_rejoin_marker_is_a_send_time_and_nothing_else() {
        assert_eq!(parse_rejoin_marker("40"), Some(40));
        assert_eq!(parse_rejoin_marker("0"), Some(0));
        assert_eq!(parse_rejoin_marker(""), None);
        assert_eq!(parse_rejoin_marker("-1"), None);
        assert_eq!(parse_rejoin_marker("40 "), None);
    }
}
