//! The time from an input to the output it causes, read from send stamps alone.
//!
//! `hypha topic delay` follows an input topic and an output topic and never decodes a
//! payload: a frame's header carries the host monotonic time at which its publisher sent
//! it, and the delay of an output frame is its stamp minus the stamp of the newest input
//! frame sent at or before it. A cloud of megabytes costs what a message of a few bytes
//! costs. On a triggered node that is the time from the trigger sample to the output; on a
//! rate node it is the age of the newest input when the step's output left, which holds
//! the wait for the next step. The stamps are one clock, so both topics must be on one host.

use std::collections::VecDeque;
use std::fmt;

use crate::approx_f64;

/// The input stamps kept: the newest ones, however fast the input is sent.
const MAX_INPUTS: usize = 65_536;
/// An input older than this behind the newest input is forgotten.
const LOOKBACK_NANOS: u64 = 5_000_000_000;

/// The delays of the last outputs of one pair of topics, and the input stamps they are
/// measured from. Memory is bounded: [`Delays::MAX_INPUTS`] stamps and one window.
#[derive(Debug, Clone)]
pub struct Delays {
    /// Input send stamps, ascending.
    inputs: VecDeque<u64>,
    /// The delays of the last outputs, oldest first.
    window: VecDeque<u64>,
    window_size: usize,
    /// Outputs sent before any input kept: they have no delay.
    unpaired: u64,
}

impl Delays {
    /// The most input stamps kept.
    pub const MAX_INPUTS: usize = MAX_INPUTS;

    /// An empty meter over the last `window_size` outputs (at least one).
    #[must_use]
    pub fn new(window_size: usize) -> Self {
        Self {
            inputs: VecDeque::new(),
            window: VecDeque::new(),
            window_size: window_size.max(1),
            unpaired: 0,
        }
    }

    /// Notes an input frame sent at the host monotonic time `sent_nanos`. Stamps of several
    /// publishers may arrive slightly out of order; they are kept in order.
    pub fn on_input(&mut self, sent_nanos: u64) {
        let at = self.inputs.partition_point(|stamp| *stamp <= sent_nanos);
        self.inputs.insert(at, sent_nanos);
        let newest = self.inputs.back().copied().unwrap_or(sent_nanos);
        let oldest_kept = newest.saturating_sub(LOOKBACK_NANOS);
        while self.inputs.len() > MAX_INPUTS
            || self.inputs.front().is_some_and(|stamp| *stamp < oldest_kept)
        {
            self.inputs.pop_front();
        }
    }

    /// Notes an output frame sent at `sent_nanos` and returns its delay: the time since the
    /// newest input sent at or before it. An output before every input kept has none; it is
    /// counted and reported as unpaired.
    pub fn on_output(&mut self, sent_nanos: u64) -> Option<u64> {
        let after = self.inputs.partition_point(|stamp| *stamp <= sent_nanos);
        let Some(input) = after.checked_sub(1).and_then(|index| self.inputs.get(index)) else {
            self.unpaired = self.unpaired.saturating_add(1);
            return None;
        };
        let delay = sent_nanos.saturating_sub(*input);
        self.window.push_back(delay);
        while self.window.len() > self.window_size {
            self.window.pop_front();
        }
        Some(delay)
    }

    /// How many input stamps are kept.
    #[must_use]
    pub fn inputs_kept(&self) -> usize {
        self.inputs.len()
    }

    /// The figures of the window, or `None` before the first delay.
    #[must_use]
    pub fn report(&self) -> Option<DelayReport> {
        let mut sorted: Vec<u64> = self.window.iter().copied().collect();
        sorted.sort_unstable();
        let min_nanos = *sorted.first()?;
        let max_nanos = *sorted.last()?;
        Some(DelayReport {
            count: sorted.len(),
            min_nanos,
            median_nanos: quantile(&sorted, 50),
            p99_nanos: quantile(&sorted, 99),
            max_nanos,
            unpaired: self.unpaired,
        })
    }
}

/// The smallest value of `sorted` (ascending, not empty) with at least `percent` percent of
/// the values at or below it.
fn quantile(sorted: &[u64], percent: usize) -> u64 {
    let rank = sorted.len().saturating_mul(percent).div_ceil(100).max(1);
    sorted.get(rank.saturating_sub(1)).or_else(|| sorted.last()).copied().unwrap_or(0)
}

/// The delay figures of one window of outputs, in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelayReport {
    /// Outputs in the window.
    pub count: usize,
    /// The shortest delay.
    pub min_nanos: u64,
    /// The median delay.
    pub median_nanos: u64,
    /// The 99th percentile.
    pub p99_nanos: u64,
    /// The longest delay.
    pub max_nanos: u64,
    /// Outputs since the start that were sent before any input kept.
    pub unpaired: u64,
}

fn ms(nanos: u64) -> f64 {
    approx_f64(nanos) / 1_000_000.0
}

impl fmt::Display for DelayReport {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            out,
            "delay: median {:.3} ms, p99 {:.3} ms, max {:.3} ms (min {:.3} ms) over {} outputs",
            ms(self.median_nanos),
            ms(self.p99_nanos),
            ms(self.max_nanos),
            ms(self.min_nanos),
            self.count
        )?;
        if self.unpaired > 0 {
            write!(out, ", {} outputs had no input before them", self.unpaired)?;
        }
        Ok(())
    }
}
