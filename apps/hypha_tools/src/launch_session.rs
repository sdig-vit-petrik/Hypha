//! tmux session layout for `hypha launch up`.
//!
//! These functions name tmux commands as argument vectors and run nothing.
//! [`crate::launch_tmux`] runs them.

use crate::launch_units::{ParsedUnits, is_valid_launch_name};
use crate::theme::{ACCENT, ERR, FG, MUTED, OK, ON_ACCENT, RAISED, SURFACE, hex};
use std::path::Path;

/// Name of the window that keeps the session open while unit windows are created.
pub const SETUP_WINDOW_NAME: &str = "hypha_launch_setup";

/// Status line of a launch session in the Mycelium palette ([`crate::theme`]): the
/// session name on the left, host and time on the right. Session options, so the
/// user's own tmux configuration is untouched.
#[must_use]
pub fn session_theme() -> Vec<(&'static str, String)> {
    let (surface, raised, fg, muted) = (hex(SURFACE), hex(RAISED), hex(FG), hex(MUTED));
    let (accent, on_accent) = (hex(ACCENT), hex(ON_ACCENT));
    vec![
        ("status-style", format!("bg={surface},fg={fg}")),
        (
            "status-left",
            format!(
                "#[bg={accent},fg={on_accent},bold] hypha #[bg={raised},fg={fg},nobold] #S #[default] "
            ),
        ),
        ("status-left-length", "40".to_string()),
        ("status-right", format!("#[fg={muted}]#H #[fg={fg}]%H:%M ")),
        ("message-style", format!("bg={raised},fg={fg}")),
        ("window-status-separator", String::new()),
    ]
}

/// Window list entries of a unit window: a unit whose process ended for good shows
/// in `ERR` with a cross, the current window is highlighted in `OK`. A `#` inside a
/// `#{?…}` conditional is written `##`.
#[must_use]
pub fn window_theme() -> Vec<(&'static str, String)> {
    let (err, ok, muted, on_accent) = (hex(ERR), hex(OK), hex(MUTED), hex(ON_ACCENT));
    vec![
        (
            "window-status-format",
            format!("#{{?pane_dead,#[fg=#{err}],#[fg=#{muted}]}} #I #W#{{?pane_dead, ✗,}} "),
        ),
        (
            "window-status-current-format",
            format!(
                "#{{?pane_dead,#[bg=#{err}],#[bg=#{ok}]}}#[fg={on_accent},bold] #I #W#{{?pane_dead, ✗,}} "
            ),
        ),
    ]
}

/// Converts borrowed arguments into one owned tmux argument vector.
#[must_use]
pub fn tmux_args(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

/// Session name for `units_file`: `requested` when given, else the file stem.
///
/// # Errors
///
/// Returns an error when the name is empty or holds a character other than
/// an ASCII letter, digit, `_` or `-`.
pub fn session_name(units_file: &Path, requested: Option<&str>) -> Result<String, String> {
    let stem = units_file.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
    let name = requested.unwrap_or(stem);
    if !is_valid_launch_name(name) {
        return Err(format!(
            "session name '{name}' must hold only ASCII letters, digits, '_' and '-'; choose one with --session"
        ));
    }
    Ok(name.to_string())
}

/// tmux commands that build the session for `parsed`, in order.
///
/// Each unit waits at a tmux barrier until `remain-on-exit` is set, then execs
/// `hypha launch unit` (`launcher` is the `hypha` executable). The history limit is set
/// before any unit window exists, because tmux applies it when a pane is created.
///
/// The session is created with `environment`; a tmux server that is already running
/// would otherwise give the units its own environment.
///
/// # Errors
///
/// Returns an error when a path is not UTF-8 or a unit uses the setup window name.
pub fn up_commands(
    parsed: &ParsedUnits,
    session: &str,
    launcher: &Path,
    units_file: &Path,
    environment: &[(String, String)],
) -> Result<Vec<Vec<String>>, String> {
    let launcher_text = utf8_path(launcher)?;
    let units_file_text = utf8_path(units_file)?;
    let session_target = format!("={session}:");
    let history_limit = parsed.history_limit.to_string();

    // One set-environment per variable: tmux refuses a command over 16 KiB, and a whole
    // environment passed as `new-session -e` flags exceeds that on CI hosts.
    let mut commands =
        vec![tmux_args(&["new-session", "-d", "-s", session, "-n", SETUP_WINDOW_NAME, "cat"])];
    for (name, value) in environment {
        commands.push(tmux_args(&["set-environment", "-t", &session_target, name, value]));
    }
    commands.push(tmux_args(&[
        "set-option",
        "-t",
        &session_target,
        "history-limit",
        &history_limit,
    ]));
    for (option, value) in session_theme() {
        commands.push(tmux_args(&["set-option", "-t", &session_target, option, &value]));
    }
    let window_theme = window_theme();
    for &unit_index in &parsed.start_order {
        let Some(unit) = parsed.units.get(unit_index) else {
            return Err(format!("start order names unit index {unit_index}, which does not exist"));
        };
        if unit.name == SETUP_WINDOW_NAME {
            return Err(format!("unit name '{SETUP_WINDOW_NAME}' is reserved by hypha launch"));
        }
        let window_target = format!("={session}:={}", unit.name);
        let channel = format!("hypha_launch_{}_{session}_{}", std::process::id(), unit.name);
        commands.push(tmux_args(&[
            "new-window",
            "-d",
            "-t",
            &session_target,
            "-n",
            &unit.name,
            "/bin/sh",
            "-c",
            r#"tmux wait-for "$1" || exit; shift; exec "$@""#,
            "hypha_launch_start",
            &channel,
            launcher_text,
            "launch",
            "unit",
            units_file_text,
            &unit.name,
        ]));
        commands.push(tmux_args(&[
            "set-option",
            "-w",
            "-t",
            &window_target,
            "remain-on-exit",
            "on",
        ]));
        for (option, value) in &window_theme {
            commands.push(tmux_args(&["set-option", "-w", "-t", &window_target, option, value]));
        }
        commands.push(tmux_args(&["wait-for", "-S", &channel]));
    }
    let setup_target = format!("={session}:={SETUP_WINDOW_NAME}");
    commands.push(tmux_args(&["kill-window", "-t", &setup_target]));
    Ok(commands)
}

fn utf8_path(path: &Path) -> Result<&str, String> {
    path.to_str().ok_or_else(|| format!("path '{}' is not valid UTF-8", path.display()))
}
