//! The run of loans a publisher's pool refused, reported once when it lasts.

/// One run of loans the pool refused below the publisher's own loan limit.
#[derive(Debug, Default)]
pub(crate) struct RefusalEpisode {
    since: Option<std::time::Instant>,
    warned: bool,
}

impl RefusalEpisode {
    /// How long refusals last before the warning.
    const PERSISTS: std::time::Duration = std::time::Duration::from_secs(1);

    /// Record one loan. True when this refusal is the first past [`Self::PERSISTS`]
    /// in its run. A granted loan ends the run.
    pub(crate) fn observe(
        &mut self,
        refused: bool,
        now: impl FnOnce() -> std::time::Instant,
    ) -> bool {
        if !refused {
            *self = Self::default();
            return false;
        }
        let now = now();
        let since = *self.since.get_or_insert(now);
        if self.warned || now.saturating_duration_since(since) <= Self::PERSISTS {
            return false;
        }
        self.warned = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::RefusalEpisode;
    use std::time::{Duration, Instant};

    #[test]
    fn a_run_longer_than_a_second_is_reported_once_and_a_granted_loan_ends_it() {
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        let mut episode = RefusalEpisode::default();
        assert!(!episode.observe(true, || at(0)));
        assert!(!episode.observe(true, || at(1000)));
        assert!(episode.observe(true, || at(1001)));
        assert!(!episode.observe(true, || at(5000)), "one warning per run");
        assert!(!episode.observe(false, || at(5001)));
        assert!(!episode.observe(true, || at(5002)));
        assert!(!episode.observe(true, || at(6002)));
        assert!(episode.observe(true, || at(6003)), "a new run is reported again");
    }

    #[test]
    fn short_runs_separated_by_granted_loans_are_never_reported() {
        let start = Instant::now();
        let mut episode = RefusalEpisode::default();
        for second in 0..10 {
            let now = start + Duration::from_secs(second);
            assert!(!episode.observe(true, || now));
            assert!(!episode.observe(true, || now + Duration::from_millis(900)));
            assert!(!episode.observe(false, || now + Duration::from_millis(950)));
        }
    }
}
