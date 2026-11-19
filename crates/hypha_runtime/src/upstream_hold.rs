//! The upstream hold of a replay cycle, as pure rules.
//!
//! A cycle runs a deadline only when the nodes it reads from have run every deadline
//! before it, so a feedback loop does not deadlock. A deadline that must wait is kept
//! as a backlog, so the timer never sees the wait as a rewind.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// How often a stalled replay cycle re-resolves its upstream names to drop killed nodes.
pub const RESOLVE_WHILE_STALLED: Duration = Duration::from_millis(250);

/// Consecutive deadlines of one replay cycle that have been counted but not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// The first deadline of the window, in nanoseconds.
    pub first_nanos: i64,
    /// The number of consecutive deadlines in the window.
    pub periods: u64,
}

/// The deadlines the upstream nodes allow: those at or before `at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limit {
    /// The newest deadline that may run, or `None` when nothing is held.
    pub at: Option<i64>,
    /// The upstream node that holds the limit, with the last clock it reported.
    pub blocking: Option<(String, Option<u64>)>,
}

/// What a replay turn does with its deadlines.
#[derive(Debug, PartialEq, Eq)]
pub enum Admit {
    /// Run these deadlines now. Zero periods means nothing runs.
    Run(Window),
    /// Keep every deadline held, and try again on a later turn; no time limit of its own.
    Hold,
}

/// A refusal of the hold. Each one is an invalid state of the cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldError {
    /// The period is not positive.
    InvalidPeriod,
    /// A window does not continue the held deadlines without a gap.
    Gap,
    /// A deadline or a count is not representable.
    Overflow,
}

/// The held deadlines of one replay cycle and the upstream nodes it waits for.
#[derive(Default)]
pub struct UpstreamHold {
    /// The upstream node names, sorted. Empty means nothing is held.
    names: Vec<String>,
    /// The publisher node ids the names were resolved for: the inputs', then the
    /// progress input's.
    resolved_for: Option<(Vec<u128>, Vec<u128>)>,
    backlog: Option<Window>,
    /// The last deadline counted by the newest admission, run or held.
    counted_last: Option<i64>,
    stalled_since: Option<Instant>,
    last_forced_resolve: Option<Instant>,
}

impl UpstreamHold {
    /// Resolve the upstream names of the cycle from the publisher node ids of its inputs
    /// and of `/clock_progress`. The names are those in both sets, without `own_name`.
    ///
    /// When the hold is stalled, the names are re-resolved at most once per
    /// [`RESOLVE_WHILE_STALLED`], so a dead node drops out.
    ///
    /// # Errors
    /// Returns the error of `names_of`.
    pub fn resolve<E>(
        &mut self,
        input_ids: &[u128],
        progress_ids: &[u128],
        own_name: &str,
        now: Instant,
        names_of: impl FnOnce(&[u128]) -> Result<BTreeMap<u128, String>, E>,
    ) -> Result<(), E> {
        if self.stalled_since.is_some() {
            let re_resolve = self
                .last_forced_resolve
                .is_none_or(|last| now.saturating_duration_since(last) >= RESOLVE_WHILE_STALLED);
            if re_resolve {
                self.resolved_for = None;
                self.last_forced_resolve = Some(now);
            }
        }
        let resolved = self
            .resolved_for
            .as_ref()
            .is_some_and(|(inputs, progress)| inputs == input_ids && progress == progress_ids);
        if resolved {
            return Ok(());
        }
        let mut ids: Vec<u128> = input_ids.iter().chain(progress_ids).copied().collect();
        ids.sort_unstable();
        ids.dedup();
        let names = names_of(&ids)?;
        let inputs_named: BTreeSet<String> =
            input_ids.iter().filter_map(|id| names.get(id).cloned()).collect();
        let reporters_named: BTreeSet<String> =
            progress_ids.iter().filter_map(|id| names.get(id).cloned()).collect();
        self.names = inputs_named
            .intersection(&reporters_named)
            .filter(|name| name.as_str() != own_name)
            .cloned()
            .collect();
        self.resolved_for = Some((input_ids.to_vec(), progress_ids.to_vec()));
        Ok(())
    }

    /// Forget the resolved names, after the progress input is gone.
    pub fn clear_resolution(&mut self) {
        self.names.clear();
        self.resolved_for = None;
        self.last_forced_resolve = None;
    }

    /// The limit while the progress input is busy: the names resolved last, or a hold
    /// of every deadline.
    #[must_use]
    pub fn limit_while_busy(&self, clocks: impl Fn(&str) -> (Option<u64>, Option<u64>)) -> Limit {
        if self.resolved_for.is_some() {
            return self.limit(clocks);
        }
        Limit {
            at: Some(i64::MIN),
            blocking: Some(("the progress input, which is busy".to_owned(), None)),
        }
    }

    /// The limit that the upstream names give now.
    #[must_use]
    pub fn limit(&self, clocks: impl Fn(&str) -> (Option<u64>, Option<u64>)) -> Limit {
        limit_over(&self.names, clocks)
    }

    /// Merge `new` with the held deadlines, run what `limit` allows, and hold the rest.
    ///
    /// `new` must continue the held deadlines without a gap.
    ///
    /// # Errors
    /// Returns a [`HoldError`] for a period that is not positive, a window that does not
    /// follow the held one, or a value that is not representable.
    pub fn admit(
        &mut self,
        new: Option<Window>,
        period: i64,
        limit: &Limit,
        now: Instant,
    ) -> Result<Admit, HoldError> {
        if period <= 0 {
            return Err(HoldError::InvalidPeriod);
        }
        let Some(window) = self.merge(new, period)? else {
            self.counted_last = None;
            self.stalled_since = None;
            self.last_forced_resolve = None;
            return Ok(Admit::Run(Window { first_nanos: 0, periods: 0 }));
        };
        self.counted_last = window
            .periods
            .checked_sub(1)
            .and_then(|last| nth_deadline(window.first_nanos, period, last));
        let (run, rest) = match limit.at {
            None => (window, None),
            Some(at) => split(window, period, at)?,
        };
        self.backlog = rest;
        match rest {
            None => {
                self.stalled_since = None;
                self.last_forced_resolve = None;
            }
            Some(_) => {
                if run.periods > 0 || self.stalled_since.is_none() {
                    self.stalled_since = Some(now);
                    self.last_forced_resolve = Some(now);
                }
            }
        }
        let held_only = rest.is_some() && run.periods == 0;
        if held_only {
            return Ok(Admit::Hold);
        }
        Ok(Admit::Run(run))
    }

    /// The first held deadline, if any; a cycle with a backlog reports up to it, exclusive.
    #[must_use]
    pub fn backlog_first(&self) -> Option<i64> {
        self.backlog.map(|window| window.first_nanos)
    }

    /// The last deadline counted by the newest admission, run or held.
    #[must_use]
    pub fn counted_last(&self) -> Option<i64> {
        self.counted_last
    }

    /// The held deadlines joined with `new`, or whichever of the two exists.
    fn merge(&mut self, new: Option<Window>, period: i64) -> Result<Option<Window>, HoldError> {
        let held = self.backlog.take();
        let (Some(held), Some(new)) = (held, new) else {
            return Ok(held.or(new));
        };
        if nth_deadline(held.first_nanos, period, held.periods) != Some(new.first_nanos) {
            return Err(HoldError::Gap);
        }
        let periods = held.periods.checked_add(new.periods).ok_or(HoldError::Overflow)?;
        Ok(Some(Window { first_nanos: held.first_nanos, periods }))
    }
}

/// The deadline `index` periods after `first_nanos`, or `None` when it is not representable.
#[must_use]
pub fn nth_deadline(first_nanos: i64, period_nanos: i64, index: u64) -> Option<i64> {
    let advance = i128::from(period_nanos).checked_mul(i128::from(index))?;
    let deadline = i128::from(first_nanos).checked_add(advance)?;
    i64::try_from(deadline).ok()
}

/// The limit of `names`: a name without a valid clock holds every deadline, otherwise
/// deadlines up to one past the lowest clock may run.
fn limit_over(names: &[String], clocks: impl Fn(&str) -> (Option<u64>, Option<u64>)) -> Limit {
    let mut lowest: Option<(String, u64)> = None;
    for name in names {
        let (valid, last) = clocks(name);
        let Some(clock) = valid else {
            return Limit { at: Some(i64::MIN), blocking: Some((name.clone(), last)) };
        };
        if lowest.as_ref().is_none_or(|(_, low)| clock < *low) {
            lowest = Some((name.clone(), clock));
        }
    }
    match lowest {
        None => Limit { at: None, blocking: None },
        Some((name, clock)) => {
            let at = i64::try_from(clock).unwrap_or(i64::MAX).saturating_add(1);
            Limit { at: Some(at), blocking: Some((name, Some(clock))) }
        }
    }
}

/// Split `window` at `at`: the periods that run now, and the rest that stay held.
fn split(window: Window, period: i64, at: i64) -> Result<(Window, Option<Window>), HoldError> {
    if window.periods == 0 {
        return Ok((window, None));
    }
    let runs = due_by(window, period, at)?;
    let run = Window { periods: runs, ..window };
    if runs >= window.periods {
        return Ok((run, None));
    }
    let first = nth_deadline(window.first_nanos, period, runs).ok_or(HoldError::Overflow)?;
    let periods = window.periods.checked_sub(runs).ok_or(HoldError::Overflow)?;
    Ok((run, Some(Window { first_nanos: first, periods })))
}

/// How many of the periods of `window` are at or before `at`.
fn due_by(window: Window, period: i64, at: i64) -> Result<u64, HoldError> {
    if at < window.first_nanos {
        return Ok(0);
    }
    let span =
        i128::from(at).checked_sub(i128::from(window.first_nanos)).ok_or(HoldError::Overflow)?;
    let steps = span.checked_div(i128::from(period)).ok_or(HoldError::Overflow)?;
    let count = steps.checked_add(1).ok_or(HoldError::Overflow)?;
    Ok(u64::try_from(count).unwrap_or(u64::MAX).min(window.periods))
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects, clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const PERIOD: i64 = 10;

    fn window(first_nanos: i64, periods: u64) -> Window {
        Window { first_nanos, periods }
    }

    fn at(value: Option<i64>) -> Limit {
        Limit { at: value, blocking: Some(("up".to_owned(), Some(0))) }
    }

    #[test]
    fn deadlines_up_to_the_limit_run_and_the_rest_are_held_then_resume() {
        let now = Instant::now();
        let mut hold = UpstreamHold::default();
        let run = hold.admit(Some(window(100, 5)), PERIOD, &at(Some(125)), now);
        assert_eq!(run.unwrap(), Admit::Run(window(100, 3)));
        assert_eq!(hold.backlog_first(), Some(130));
        assert_eq!(hold.admit(None, PERIOD, &at(Some(99)), now).unwrap(), Admit::Hold);
        assert_eq!(hold.backlog_first(), Some(130));
        let resumed = hold.admit(None, PERIOD, &at(None), now);
        assert_eq!(resumed.unwrap(), Admit::Run(window(130, 2)));
        assert_eq!(hold.backlog_first(), None);
    }

    #[test]
    fn a_limit_at_a_deadline_runs_that_deadline() {
        let now = Instant::now();
        let mut hold = UpstreamHold::default();
        let run = hold.admit(Some(window(100, 5)), PERIOD, &at(Some(140)), now);
        assert_eq!(run.unwrap(), Admit::Run(window(100, 5)));
        assert_eq!(hold.backlog_first(), None);
    }

    #[test]
    fn a_held_window_merges_with_a_contiguous_window() {
        let now = Instant::now();
        let mut hold = UpstreamHold::default();
        let run = hold.admit(Some(window(100, 5)), PERIOD, &at(Some(105)), now);
        assert_eq!(run.unwrap(), Admit::Run(window(100, 1)));
        assert_eq!(hold.backlog_first(), Some(110));
        let merged = hold.admit(Some(window(150, 2)), PERIOD, &at(Some(140)), now);
        assert_eq!(merged.unwrap(), Admit::Run(window(110, 4)));
        assert_eq!(hold.backlog_first(), Some(150));
    }

    #[test]
    fn a_window_that_does_not_follow_the_held_one_is_refused() {
        let now = Instant::now();
        let mut hold = UpstreamHold::default();
        hold.admit(Some(window(100, 5)), PERIOD, &at(Some(105)), now).unwrap();
        let refused = hold.admit(Some(window(160, 1)), PERIOD, &at(None), now);
        assert_eq!(refused, Err(HoldError::Gap));
    }

    #[test]
    fn a_period_that_is_not_positive_is_refused() {
        let now = Instant::now();
        let mut hold = UpstreamHold::default();
        let refused = hold.admit(Some(window(100, 5)), 0, &at(None), now);
        assert_eq!(refused, Err(HoldError::InvalidPeriod));
    }

    #[test]
    fn no_limit_runs_everything_and_the_lowest_limit_holds_everything() {
        let now = Instant::now();
        let mut hold = UpstreamHold::default();
        let outcome = hold.admit(Some(window(100, 5)), PERIOD, &at(Some(i64::MIN)), now);
        assert_eq!(outcome.unwrap(), Admit::Hold);
        let all = hold.admit(None, PERIOD, &at(None), now);
        assert_eq!(all.unwrap(), Admit::Run(window(100, 5)));
    }

    #[test]
    fn a_held_window_holds_with_no_time_limit() {
        let start = Instant::now();
        let mut hold = UpstreamHold::default();
        let stuck = at(Some(i64::MIN));
        assert_eq!(hold.admit(Some(window(100, 5)), PERIOD, &stuck, start).unwrap(), Admit::Hold);
        let an_hour_later = start + Duration::from_secs(3_600);
        assert_eq!(hold.admit(None, PERIOD, &stuck, an_hour_later).unwrap(), Admit::Hold);
        let progressed = hold.admit(None, PERIOD, &at(Some(100)), an_hour_later);
        assert_eq!(progressed.unwrap(), Admit::Run(window(100, 1)));
    }

    #[test]
    fn the_limit_follows_the_lowest_valid_clock_and_a_missing_one_holds_all() {
        let names = ["a".to_owned(), "b".to_owned()];
        let lowest = limit_over(&names, |name| match name {
            "a" => (Some(500), Some(500)),
            _ => (Some(300), Some(300)),
        });
        assert_eq!(lowest, Limit { at: Some(301), blocking: Some(("b".to_owned(), Some(300))) });
        let missing = limit_over(&names, |name| match name {
            "a" => (None, Some(200)),
            _ => (Some(300), Some(300)),
        });
        assert_eq!(
            missing,
            Limit { at: Some(i64::MIN), blocking: Some(("a".to_owned(), Some(200))) }
        );
        let never = limit_over(&names, |_| (None, None));
        assert_eq!(never, Limit { at: Some(i64::MIN), blocking: Some(("a".to_owned(), None)) });
        assert_eq!(limit_over(&[], |_| (None, None)), Limit { at: None, blocking: None });
    }

    #[test]
    fn a_resolution_names_the_nodes_that_both_read_and_report_except_this_one() {
        let names = BTreeMap::from([
            (1_u128, "up".to_owned()),
            (2_u128, "me".to_owned()),
            (3_u128, "reader_only".to_owned()),
        ]);
        let mut hold = UpstreamHold::default();
        hold.resolve(&[1, 2, 3], &[1, 2], "me", Instant::now(), |_| Ok::<_, ()>(names)).unwrap();
        let limit = hold.limit(|name| {
            assert_eq!(name, "up");
            (Some(50), Some(50))
        });
        assert_eq!(limit, Limit { at: Some(51), blocking: Some(("up".to_owned(), Some(50))) });
    }

    #[test]
    fn a_failed_resolution_leaves_the_hold_busy_and_unresolved() {
        let mut hold = UpstreamHold::default();
        let failed = hold.resolve(&[1], &[1], "me", Instant::now(), |_| Err("refused"));
        assert_eq!(failed, Err("refused"));
        let busy = hold.limit_while_busy(|_| (None, None));
        assert_eq!(
            busy,
            Limit {
                at: Some(i64::MIN),
                blocking: Some(("the progress input, which is busy".to_owned(), None)),
            }
        );
    }

    #[test]
    fn the_counted_end_is_the_end_of_the_held_window_not_of_the_run() {
        let now = Instant::now();
        let mut hold = UpstreamHold::default();
        hold.admit(Some(window(100, 5)), PERIOD, &at(Some(125)), now).unwrap();
        assert_eq!(hold.counted_last(), Some(140));
    }

    #[test]
    fn a_stalled_hold_re_resolves_upstream_names_after_the_interval() {
        let start = Instant::now();
        let mut hold = UpstreamHold::default();
        let calls = std::cell::Cell::new(0);
        let resolve = |hold: &mut UpstreamHold, now: Instant, dead: bool| {
            hold.resolve(&[1], &[1], "me", now, |_| {
                calls.set(calls.get() + 1);
                Ok::<_, ()>(if dead {
                    BTreeMap::new()
                } else {
                    BTreeMap::from([(1, "dead".into())])
                })
            })
        };
        resolve(&mut hold, start, false).unwrap();
        assert_eq!(calls.get(), 1);
        let limit = hold.limit(|_| (None, None));
        let outcome = hold.admit(Some(window(100, 2)), PERIOD, &limit, start).unwrap();
        assert_eq!(outcome, Admit::Hold);
        resolve(&mut hold, start + Duration::from_millis(100), true).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(hold.names, vec!["dead".to_owned()]);
        let at_interval = start + RESOLVE_WHILE_STALLED;
        resolve(&mut hold, at_interval, true).unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(hold.names, Vec::<String>::new());
        let unblocked = hold.limit(|_| (None, None));
        let resumed = hold.admit(None, PERIOD, &unblocked, at_interval).unwrap();
        assert_eq!(resumed, Admit::Run(window(100, 2)));
        assert_eq!(hold.backlog_first(), None);
        assert!(hold.stalled_since.is_none());
        assert!(hold.last_forced_resolve.is_none());
    }
}
