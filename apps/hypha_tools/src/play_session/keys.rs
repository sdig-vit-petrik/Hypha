//! Keyboard handling and the progress line, plus the turn of waiting that serves both.

use super::{PROGRESS_RENDER_INTERVAL, Playback, Session};
use crate::approx_f64;
use crate::replay_sync::{Pace, faster, slower};
use crate::terminal_ui::KeyAction;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

impl Session<'_> {
    /// One turn of waiting: serve the executor, read the reports, handle a key, and
    /// draw the progress line when it is due.
    pub(super) fn idle(&mut self, wait: Duration) -> Result<(), String> {
        self.serve(wait)?;
        let action = self.term.poll_key();
        self.apply_key(action);
        self.render_if_due();
        Ok(())
    }

    pub(super) fn apply_key(&mut self, action: KeyAction) {
        let now = Instant::now();
        let changes_schedule = matches!(
            action,
            KeyAction::PauseToggle | KeyAction::Step | KeyAction::SpeedUp | KeyAction::SlowDown
        );
        if changes_schedule
            && self.playback == Playback::Running
            && let Some(factor) = self.config.pace.factor()
        {
            self.schedule.freeze(now, factor);
        }
        match action {
            KeyAction::PauseToggle => {
                if self.playback == Playback::Running {
                    self.playback = Playback::Paused;
                } else {
                    // The schedule was frozen when the replay paused: resume from there.
                    let anchor = self.schedule.anchor_log;
                    self.schedule.reanchor(anchor, now);
                    self.playback = Playback::Running;
                }
            }
            KeyAction::Step => {
                if matches!(self.playback, Playback::Running | Playback::Paused) {
                    self.playback = Playback::StepRequested;
                }
            }
            KeyAction::SpeedUp => {
                if let Pace::Factor(factor) = self.config.pace {
                    self.config.pace = Pace::Factor(faster(factor));
                }
            }
            KeyAction::SlowDown => {
                if let Pace::Factor(factor) = self.config.pace {
                    self.config.pace = Pace::Factor(slower(factor));
                }
            }
            KeyAction::Restart => self.restart_requested = true,
            KeyAction::Quit => self.stop_signal.store(true, Ordering::Relaxed),
            KeyAction::None => {}
        }
    }

    pub(super) fn render_if_due(&mut self) {
        let now = Instant::now();
        if now.saturating_duration_since(self.last_render) < PROGRESS_RENDER_INTERVAL {
            return;
        }
        self.last_render = now;
        let elapsed_log = approx_f64(self.last_log_time.saturating_sub(self.pass_start_log)) / 1e9;
        let topic = Some(self.last_topic.as_str()).filter(|topic| !topic.is_empty());
        self.term.render_progress(
            elapsed_log,
            self.total_duration,
            &self.config.pace.to_string(),
            self.playback != Playback::Running,
            self.current_msg,
            self.total_messages,
            topic,
        );
    }
}
