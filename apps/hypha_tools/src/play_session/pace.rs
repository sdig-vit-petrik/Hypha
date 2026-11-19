//! Pacing: the wall-clock schedule of a pass, and the waits that hold a replay to it.

use super::{CONTROL_POLL_INTERVAL, Flow, Playback, Session};
use crate::approx_f64;
use std::time::{Duration, Instant};

/// The wall-clock schedule of a pass: log time `anchor_log` is due at `anchor_wall`,
/// and log time runs at the pace from there.
#[derive(Debug, Clone, Copy)]
pub(super) struct Schedule {
    pub(super) anchor_log: u64,
    pub(super) anchor_wall: Instant,
}

impl Schedule {
    pub(super) fn reanchor(&mut self, log_time: u64, now: Instant) {
        self.anchor_log = log_time;
        self.anchor_wall = now;
    }

    /// The log time that the schedule has reached at `now`, or `None` when it does not
    /// fit a `u64`.
    pub(super) fn virtual_log_time(&self, now: Instant, rate: f64) -> Option<u64> {
        let elapsed = now.saturating_duration_since(self.anchor_wall).as_secs_f64() * rate;
        let elapsed = Duration::try_from_secs_f64(elapsed).ok()?;
        let elapsed = u64::try_from(elapsed.as_nanos()).ok()?;
        self.anchor_log.checked_add(elapsed)
    }

    /// Holds the schedule at its position at `now`. Call before the rate changes or the
    /// replay pauses: a new slope without a new anchor would move every later deadline.
    pub(super) fn freeze(&mut self, now: Instant, rate: f64) {
        if let Some(virtual_time) = self.virtual_log_time(now, rate) {
            self.reanchor(virtual_time, now);
        }
    }

    /// The wall time at which `log_time` is due, or `None` for a rate that cannot pace.
    pub(super) fn deadline(&self, log_time: u64, rate: f64) -> Option<Instant> {
        let elapsed_log_nanos = log_time.saturating_sub(self.anchor_log);
        let elapsed_wall_seconds = approx_f64(elapsed_log_nanos) / 1e9 / rate;
        if !elapsed_wall_seconds.is_finite() || elapsed_wall_seconds < 0.0 {
            return None;
        }
        // A rate far below one overflows the wait: that is refused, not a panic.
        let wait = Duration::try_from_secs_f64(elapsed_wall_seconds).ok()?;
        self.anchor_wall.checked_add(wait)
    }
}

impl Session<'_> {
    /// Holds while paused. A requested step lets one chunk through.
    pub(super) fn hold_while_paused(&mut self) -> Result<Flow, String> {
        loop {
            if self.restart_requested || self.stopping() {
                return Ok(Flow::Halt);
            }
            match self.playback {
                Playback::Running | Playback::Stepping => return Ok(Flow::Go),
                Playback::StepRequested => {
                    self.playback = Playback::Stepping;
                    return Ok(Flow::Go);
                }
                Playback::Paused => {
                    let paused_at = Instant::now();
                    self.idle(CONTROL_POLL_INTERVAL)?;
                    self.report.add_paused(paused_at.elapsed());
                }
            }
        }
    }

    /// Waits, while paused or until the schedule reaches `log_time`, before anything at
    /// `log_time` is sent. A replay with no pace never waits on the schedule.
    pub(super) fn pace_to(&mut self, log_time: u64) -> Result<Flow, String> {
        loop {
            if self.hold_while_paused()? == Flow::Halt {
                return Ok(Flow::Halt);
            }
            let Some(factor) = self.config.pace.factor() else {
                return Ok(Flow::Go);
            };
            let now = Instant::now();
            let deadline = self.schedule.deadline(log_time, factor).ok_or_else(|| {
                format!(
                    "the replay cannot pace at {}: its schedule does not fit a time",
                    self.config.pace
                )
            })?;
            if now >= deadline {
                return Ok(Flow::Go);
            }
            let wait = deadline.saturating_duration_since(now).min(CONTROL_POLL_INTERVAL);
            let waiting_at = Instant::now();
            self.idle(wait)?;
            self.report.add_pace_wait(waiting_at.elapsed());
        }
    }
}
