//! Runs tmux for `hypha launch`: version check, and the `up`, `down` and
//! `attach` operations on one session.

use crate::launch_session::{tmux_args, up_commands};
use crate::launch_units::ParsedUnits;
use nix::errno::Errno;
use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::Pid;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

/// Oldest tmux release with argument-vector commands.
pub const MIN_TMUX_VERSION: (u32, u32) = (3, 0);

/// Default wait of `down`: the supervisor grace period plus time for the
/// supervisor itself to exit.
pub const DOWN_WAIT_LIMIT: Duration = Duration::from_secs(10);

const DOWN_CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// Runs the `tmux` binary on the default server or on a named socket.
#[derive(Debug, Clone, Default)]
pub struct Tmux {
    socket_name: Option<String>,
}

impl Tmux {
    /// The default tmux server of the user.
    #[must_use]
    pub fn default_server() -> Self {
        Self { socket_name: None }
    }

    /// A separate tmux server on socket `name` (`tmux -L name`).
    #[must_use]
    pub fn with_socket_name(name: &str) -> Self {
        Self { socket_name: Some(name.to_string()) }
    }

    fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        if let Some(socket_name) = &self.socket_name {
            command.arg("-L").arg(socket_name);
        }
        command
    }

    /// Runs `tmux <args>` and returns its standard output.
    ///
    /// # Errors
    ///
    /// Returns an error when tmux cannot start or exits with a failure.
    pub fn run(&self, args: &[String]) -> Result<String, String> {
        let output = self
            .command()
            .args(args)
            .output()
            .map_err(|error| format!("cannot run tmux: {error}"))?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        Err(format!(
            "tmux {}: {} ({})",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim(),
            output.status
        ))
    }

    /// Returns true when the server holds a session named exactly `session`.
    ///
    /// # Errors
    ///
    /// Returns an error when tmux cannot start or fails for another reason.
    pub fn session_exists(&self, session: &str) -> Result<bool, String> {
        let output = self
            .command()
            .args(["has-session", "-t", &format!("={session}")])
            .output()
            .map_err(|error| format!("cannot run tmux: {error}"))?;
        if output.status.success() {
            return Ok(true);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let session_is_absent = stderr.contains("can't find session")
            || stderr.contains("no server running")
            || stderr.contains("error connecting to");
        if session_is_absent {
            return Ok(false);
        }
        Err(format!("tmux has-session: {}", stderr.trim()))
    }

    /// Refuses a missing tmux or one older than [`MIN_TMUX_VERSION`].
    ///
    /// # Errors
    ///
    /// Returns an error that names the version found, or `none`.
    pub fn require_supported_version(&self) -> Result<(), String> {
        let found = match self.command().arg("-V").output() {
            Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_string(),
            Err(error) if error.kind() == ErrorKind::NotFound => "none".to_string(),
            Err(error) => return Err(format!("cannot run tmux -V: {error}")),
        };
        let supported =
            parse_tmux_version(&found).is_some_and(|version| version >= MIN_TMUX_VERSION);
        if supported {
            return Ok(());
        }
        Err(format!("tmux 3.0 or newer is required (found: {found})"))
    }
}

/// Reads `(major, minor)` from `tmux -V` output such as `tmux 3.3a`.
#[must_use]
pub fn parse_tmux_version(text: &str) -> Option<(u32, u32)> {
    let version = text.trim().strip_prefix("tmux ")?;
    let version = version.strip_prefix("next-").unwrap_or(version);
    let (major_text, rest) = version.split_once('.')?;
    let minor_text: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((major_text.parse().ok()?, minor_text.parse().ok()?))
}

/// Builds the session for `parsed`. Builds the full session or none.
///
/// # Errors
///
/// Returns an error when tmux is not supported, the session exists, or a tmux
/// command fails. A partial session is removed before the error returns.
pub fn session_up(
    tmux: &Tmux,
    parsed: &ParsedUnits,
    session: &str,
    launcher: &Path,
    units_file: &Path,
) -> Result<(), String> {
    tmux.require_supported_version()?;
    if tmux.session_exists(session)? {
        return Err(format!("session '{session}' already exists; run 'hypha launch down' first"));
    }
    let commands = up_commands(parsed, session, launcher, units_file, &caller_environment()?)?;
    let Some((create_session, remaining_commands)) = commands.split_first() else {
        return Err("the session layout holds no tmux command".to_string());
    };
    tmux.run(create_session)?;
    for command in remaining_commands {
        let Err(step_error) = tmux.run(command) else {
            continue;
        };
        let cleanup = tmux.run(&tmux_args(&["kill-session", "-t", &format!("={session}")]));
        return Err(match cleanup {
            Ok(_) => step_error,
            Err(cleanup_error) => {
                format!("{step_error}; removing the partial session also failed: {cleanup_error}")
            }
        });
    }
    Ok(())
}

/// The environment of this process, for the session. `TMUX` and `TMUX_PANE` name the
/// caller's own tmux client and are left out.
///
/// # Errors
/// Returns an error naming a variable whose name or value is not UTF-8.
fn caller_environment() -> Result<Vec<(String, String)>, String> {
    let mut environment = Vec::new();
    for (name, value) in std::env::vars_os() {
        let (Some(name_text), Some(value_text)) = (name.to_str(), value.to_str()) else {
            return Err(format!(
                "environment variable {} is not UTF-8; tmux cannot pass it to the units",
                name.to_string_lossy()
            ));
        };
        if name_text != "TMUX" && name_text != "TMUX_PANE" {
            environment.push((name_text.to_string(), value_text.to_string()));
        }
    }
    Ok(environment)
}

struct PaneProcess {
    pid: Pid,
    window_name: String,
}

/// Stops every unit of `session`, then removes the session.
///
/// # Errors
///
/// Returns an error when tmux is not supported, the session does not exist,
/// or tmux fails.
pub fn session_down(tmux: &Tmux, session: &str, wait_limit: Duration) -> Result<(), String> {
    tmux.require_supported_version()?;
    if !tmux.session_exists(session)? {
        return Err(format!("no session '{session}'"));
    }
    let panes = live_pane_processes(tmux, session)?;
    for pane in &panes {
        send_to_process(pane, Signal::SIGTERM);
    }
    let deadline = Instant::now()
        .checked_add(wait_limit)
        .ok_or_else(|| format!("wait limit {wait_limit:?} is out of range"))?;
    let mut still_running = running_panes(&panes);
    while !still_running.is_empty() && Instant::now() < deadline {
        thread::sleep(DOWN_CHECK_INTERVAL);
        still_running = running_panes(&panes);
    }
    for pane in still_running {
        println!(
            "[hypha launch] '{}' did not stop in {:.1} s; sending SIGKILL to its process group",
            pane.window_name,
            wait_limit.as_secs_f64()
        );
        match killpg(pane.pid, Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => {}
            Err(error) => println!("[hypha launch] cannot kill '{}': {error}", pane.window_name),
        }
    }
    tmux.run(&tmux_args(&["kill-session", "-t", &format!("={session}")]))?;
    Ok(())
}

/// Attaches the terminal to `session`, or switches to it from inside tmux.
///
/// # Errors
///
/// Returns an error when tmux is not supported, the session does not exist,
/// or tmux ends with a failure.
pub fn session_attach(tmux: &Tmux, session: &str) -> Result<(), String> {
    tmux.require_supported_version()?;
    if !tmux.session_exists(session)? {
        return Err(format!("no session '{session}'"));
    }
    let inside_tmux = std::env::var_os("TMUX").is_some();
    let verb = if inside_tmux { "switch-client" } else { "attach-session" };
    let status = tmux
        .command()
        .args([verb, "-t", &format!("={session}")])
        .status()
        .map_err(|error| format!("cannot run tmux {verb}: {error}"))?;
    if status.success() {
        return Ok(());
    }
    Err(format!("tmux {verb} ended with {status}"))
}

fn live_pane_processes(tmux: &Tmux, session: &str) -> Result<Vec<PaneProcess>, String> {
    let listing = tmux.run(&tmux_args(&[
        "list-panes",
        "-s",
        "-t",
        &format!("={session}"),
        "-F",
        "#{pane_dead} #{pane_pid} #{window_name}",
    ]))?;
    let mut panes = Vec::new();
    for line in listing.lines() {
        let mut fields = line.splitn(3, ' ');
        let (Some(dead), Some(pid_text), Some(window_name)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(format!("tmux list-panes printed an unexpected line: '{line}'"));
        };
        if dead == "1" {
            continue;
        }
        let raw_pid = pid_text
            .parse::<i32>()
            .map_err(|error| format!("tmux list-panes printed pid '{pid_text}': {error}"))?;
        panes.push(PaneProcess {
            pid: Pid::from_raw(raw_pid),
            window_name: window_name.to_string(),
        });
    }
    Ok(panes)
}

fn running_panes(panes: &[PaneProcess]) -> Vec<&PaneProcess> {
    panes.iter().filter(|pane| !matches!(kill(pane.pid, None), Err(Errno::ESRCH))).collect()
}

fn send_to_process(pane: &PaneProcess, signal: Signal) {
    match kill(pane.pid, signal) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(error) => {
            println!("[hypha launch] cannot send {signal} to '{}': {error}", pane.window_name);
        }
    }
}
