//! Interactive terminal controller and styled progress bar for CLI playback/recording.

use crate::theme::{ACCENT, ACCENT2, FG, MUTED, OK, RESET, WARN, fg};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use std::io::{self, IsTerminal, Write};
use std::time::Duration;

/// Interactive key actions during tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// No key pressed.
    None,
    /// Toggle playback pause/resume (Space).
    PauseToggle,
    /// Step forward by one chunk ('s'/'S').
    Step,
    /// Speed up playback rate ('+'/'=' / Up arrow).
    SpeedUp,
    /// Slow down playback rate ('-'/'_' / Down arrow).
    SlowDown,
    /// Restart playback from beginning ('r'/'R').
    Restart,
    /// Quit playback / tool ('q'/'Q'/Esc/Ctrl+C).
    Quit,
}

/// RAII Terminal controller handling raw mode and interactive keyboard input.
pub struct TerminalController {
    is_interactive: bool,
    raw_mode_enabled: bool,
}

impl Default for TerminalController {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalController {
    /// Initializes the controller. It is interactive, and enters raw mode, when stdin and
    /// stdout are terminals and this process may read the terminal (see [`owns_terminal`]).
    #[must_use]
    pub fn new() -> Self {
        let is_interactive =
            io::stdin().is_terminal() && io::stdout().is_terminal() && owns_terminal();
        let mut controller = Self { is_interactive, raw_mode_enabled: false };
        if is_interactive {
            controller.enable_raw();
        }
        controller
    }

    /// Whether standard input/output is a terminal this process took keys from at its start.
    #[must_use]
    pub fn is_interactive(&self) -> bool {
        self.is_interactive
    }

    fn enable_raw(&mut self) {
        if !self.raw_mode_enabled && enable_raw_mode().is_ok() {
            self.raw_mode_enabled = true;
            let _ = write!(io::stdout(), "\x1b[?25l"); // Hide cursor
            let _ = io::stdout().flush();
        }
    }

    fn disable_raw(&mut self) {
        if self.raw_mode_enabled {
            let _ = write!(io::stdout(), "\x1b[?25h\x1b[0m\r\x1b[K"); // Show cursor, clear line
            let _ = io::stdout().flush();
            let _ = disable_raw_mode();
            self.raw_mode_enabled = false;
        }
    }

    /// Polls for a key event without blocking.
    pub fn poll_key(&mut self) -> KeyAction {
        // Asked each time: a shell can move the process to the background after its start.
        if !self.raw_mode_enabled || !owns_terminal() {
            return KeyAction::None;
        }

        if event::poll(Duration::from_millis(0)).unwrap_or(false)
            && let Ok(Event::Key(key_event)) = event::read()
            && key_event.kind == KeyEventKind::Press
        {
            if (matches!(key_event.code, KeyCode::Char('c' | 'C'))
                && key_event.modifiers.contains(crossterm::event::KeyModifiers::CONTROL))
                || matches!(key_event.code, KeyCode::Char('q' | 'Q') | KeyCode::Esc)
            {
                return KeyAction::Quit;
            }
            return match key_event.code {
                KeyCode::Char(' ') => KeyAction::PauseToggle,
                KeyCode::Char('s' | 'S') => KeyAction::Step,
                KeyCode::Char('+' | '=' | ']') | KeyCode::Up => KeyAction::SpeedUp,
                KeyCode::Char('-' | '_' | '[') | KeyCode::Down => KeyAction::SlowDown,
                KeyCode::Char('r' | 'R') => KeyAction::Restart,
                _ => KeyAction::None,
            };
        }
        KeyAction::None
    }

    /// Renders a real-time progress bar with time stamps, message counters, and playback rate.
    #[allow(clippy::too_many_arguments)]
    pub fn render_progress(
        &self,
        elapsed_sec: f64,
        total_sec: f64,
        rate: &str,
        paused: bool,
        current_msg: u64,
        total_msgs: u64,
        topic: Option<&str>,
    ) {
        if !self.is_interactive {
            return;
        }

        let ratio = if total_sec > 0.0 { (elapsed_sec / total_sec).clamp(0.0, 1.0) } else { 0.0 };
        let percent = ratio * 100.0;

        let cols = size().map_or(80, |(w, _)| w as usize);
        let bar_width = cols.saturating_sub(58).clamp(10, 35);
        #[allow(clippy::cast_precision_loss)]
        #[allow(clippy::cast_possible_truncation)]
        #[allow(clippy::cast_sign_loss)]
        let filled = {
            let width_f64 = bar_width as f64;
            let filled_f64 = (ratio * width_f64).round();
            (filled_f64 as usize).min(bar_width)
        };

        let bar_capacity = bar_width.saturating_add(2);
        let mut bar = String::with_capacity(bar_capacity);
        bar.push('[');
        for i in 0..bar_width {
            let filled_pos = filled.saturating_sub(1);
            match i.cmp(&filled_pos) {
                std::cmp::Ordering::Less => bar.push('='),
                std::cmp::Ordering::Equal => bar.push('>'),
                std::cmp::Ordering::Greater => bar.push(' '),
            }
        }
        bar.push(']');

        let elapsed_str = format_duration(elapsed_sec);
        let total_str = format_duration(total_sec);

        let status_str = if paused {
            format!("{}[PAUSED]{RESET}", fg(WARN, true))
        } else {
            format!("{}[PLAYING]{RESET}", fg(OK, true))
        };
        let muted = fg(MUTED, false);

        let msg_str = if total_msgs > 0 {
            format!("{muted}({current_msg}/{total_msgs} msgs){RESET}")
        } else {
            format!("{muted}({current_msg} msgs){RESET}")
        };

        let topic_str = if let Some(top) = topic {
            if cols > 100 { format!(" {}{top}{RESET}", fg(WARN, false)) } else { String::new() }
        } else {
            String::new()
        };

        let _ = write!(
            io::stdout(),
            "\r\x1b[K{}{bar}{RESET} {}{percent:5.1}%{RESET} {}[{elapsed_str} / {total_str}]{RESET} {}{rate}{RESET} {status_str} {msg_str}{topic_str}",
            fg(ACCENT, true),
            fg(OK, true),
            fg(FG, true),
            fg(ACCENT2, true),
        );
        let _ = io::stdout().flush();
    }

    /// Prints the interactive keyboard controls reminder.
    pub fn print_controls_help(&self) {
        if self.is_interactive {
            println!(
                "{}Controls: <Space> Pause/Resume  <s> Step  <+>/<-> Rate  <r> Restart  <q> Quit{RESET}",
                fg(MUTED, false)
            );
        }
    }

    /// Clears the current progress line.
    pub fn clear_progress_line(&self) {
        if self.is_interactive {
            let _ = write!(io::stdout(), "\r\x1b[K");
            let _ = io::stdout().flush();
        }
    }
}

impl Drop for TerminalController {
    fn drop(&mut self) {
        self.disable_raw();
    }
}

/// Whether this process's group is the foreground group of the terminal on stdin.
///
/// Only that group can read the terminal. In any other group (a unit of `hypha launch`, a
/// shell's background job) a read fails or stops the process, and crossterm's poll repeats
/// a failed read without limit, so the keys must not be polled at all.
fn owns_terminal() -> bool {
    nix::unistd::tcgetpgrp(io::stdin()).is_ok_and(|foreground| foreground == nix::unistd::getpgrp())
}

fn format_duration(total_sec: f64) -> String {
    let non_neg = total_sec.max(0.0);
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    let sec_integral = non_neg.floor() as u64;
    #[allow(clippy::cast_precision_loss)]
    let frac = non_neg - (sec_integral as f64);
    let tenths_value = (frac * 10.0).floor();
    #[allow(clippy::cast_possible_truncation)]
    let tenths_int = tenths_value as i32;
    let sub_sec_tenth = u32::try_from(tenths_int).unwrap_or(0);

    let hours = sec_integral / 3600;
    let mins = (sec_integral % 3600) / 60;
    let secs = sec_integral % 60;

    if hours > 0 {
        format!("{hours:02}:{mins:02}:{secs:02}.{sub_sec_tenth}")
    } else {
        format!("{mins:02}:{secs:02}.{sub_sec_tenth}")
    }
}
