//! One-unit supervisor. `hypha launch unit` runs it inside a tmux window.
//!
//! On Linux only the main thread reaps, after its last signal, so the node's pid and
//! group id stay reserved while they may be signalled.

use crate::launch_units::{RestartPolicy, UnitConfig};
use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Time a node has to exit after SIGTERM before the supervisor sends SIGKILL.
pub const STOP_GRACE_PERIOD: Duration = Duration::from_secs(5);

/// The longest delay before a restart; `restartDelaySeconds` doubles up to it.
pub const RESTART_DELAY_CAP: Duration = Duration::from_secs(30);

/// A run at least this long resets the restart count and the delay.
pub const LONG_RUN: Duration = Duration::from_secs(60);

/// Restarts allowed with no run of [`LONG_RUN`] in between; the next one is refused.
pub const RESTART_BUDGET: u32 = 10;

/// The restart count and the delay of one unit since its last long run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartBudget {
    first_delay: Duration,
    delay: Duration,
    restarts: u32,
}

impl RestartBudget {
    /// A budget whose first delay is `first_delay` (`restartDelaySeconds`).
    #[must_use]
    pub fn new(first_delay: Duration) -> Self {
        Self { first_delay, delay: first_delay, restarts: 0 }
    }

    /// Account for a run of `ran`: a run of at least [`LONG_RUN`] starts afresh.
    pub fn observe_run(&mut self, ran: Duration) {
        if ran >= LONG_RUN {
            self.delay = self.first_delay;
            self.restarts = 0;
        }
    }

    /// True when [`RESTART_BUDGET`] restarts happened since the last long run.
    #[must_use]
    pub fn exhausted(&self) -> bool {
        self.restarts >= RESTART_BUDGET
    }

    /// Count one restart. Returns the delay to wait before it and its number
    /// since the last long run, and doubles the next delay up to the cap. A
    /// first delay above the cap stays as it is.
    pub fn next_restart(&mut self) -> (Duration, u32) {
        let used = self.delay;
        self.restarts = self.restarts.saturating_add(1);
        let below_cap = self.delay < RESTART_DELAY_CAP;
        if below_cap {
            self.delay = self.delay.saturating_mul(2).min(RESTART_DELAY_CAP);
        }
        (used, self.restarts)
    }
}

/// Wait status reported for a unit whose process could not be started: exit code 127
/// in bits 8 to 15.
const SPAWN_FAILURE_WAIT_STATUS: i32 = 0x7f00;

const CHANNEL_CLOSED: &str = "the supervisor event channel closed";

/// Print one line of the supervisor. A window that closed makes the write fail; the
/// error is ignored so the supervisor still stops its node's group.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ignored = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// Starts the detached thread `hypha_launch-<name>`; the error names it as the `what` thread.
fn spawn_thread(name: &str, what: &str, run: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let builder = thread::Builder::new().name(format!("hypha_launch-{name}"));
    builder
        .spawn(run)
        .map(|_| ())
        .map_err(|error| format!("cannot start the {what} thread: {error}"))
}

enum SupervisorEvent {
    /// The node exited; it is not reaped yet (Linux).
    #[cfg(target_os = "linux")]
    NodeEnded,
    /// The node exited and was reaped by the waiter thread.
    #[cfg(not(target_os = "linux"))]
    NodeExited(ExitStatus),
    NodeWaitFailed(String),
    StopRequested(i32),
}

/// A started node: its process group (the node's pid) and, on Linux, the
/// unreaped child, which only the main thread reaps.
struct RunningNode {
    group: Pid,
    #[cfg(target_os = "linux")]
    child: Child,
}

enum RunEnd {
    Exited(ExitStatus),
    Stopped(ExitStatus),
}

/// A node refused its configuration at start (`runNode`). A restart
/// cannot fix it, so it is never restarted.
pub const CONFIG_REFUSED_EXIT_CODE: i32 = 78;

/// A node's simulated clock went back. It is restarted at once under
/// every policy, and the restart is not counted.
pub const SIMULATED_REWIND_EXIT_CODE: i32 = 75;

/// Set on the start that follows an exit 75, to the supervisor's steady time in
/// nanoseconds when it saw that exit; the node drops older `/clock` samples.
/// No other start carries it.
pub const REJOIN_VARIABLE: &str = "HYPHA_REJOIN_AFTER_NANOS";

/// What the supervisor does after a node ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// End the supervisor with the node's code.
    Stop,
    /// Start the node again at once, uncounted (exit 75).
    RestartNow,
    /// Start the node again after the budget's delay, counted.
    RestartAfterDelay,
}

/// The decision for a node that ended with `status` under `policy`: 78 never
/// restarts, 75 always restarts at once, everything else follows the policy.
#[must_use]
pub fn restart_decision(policy: RestartPolicy, status: ExitStatus) -> RestartDecision {
    let code = status.code();
    if code == Some(CONFIG_REFUSED_EXIT_CODE) {
        return RestartDecision::Stop;
    }
    if code == Some(SIMULATED_REWIND_EXIT_CODE) {
        return RestartDecision::RestartNow;
    }
    let policy_restarts = match policy {
        RestartPolicy::Always => true,
        RestartPolicy::OnFailure => !status.success(),
        RestartPolicy::Never => false,
    };
    if policy_restarts {
        return RestartDecision::RestartAfterDelay;
    }
    RestartDecision::Stop
}

/// Describes how a node ended, for example `exited 3` or `killed by SIGSEGV`.
#[must_use]
pub fn describe_exit(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exited {code}");
    }
    let Some(signal_number) = status.signal() else {
        return "exited (unknown reason)".to_string();
    };
    let name = Signal::try_from(signal_number)
        .map_or_else(|_| signal_number.to_string(), |signal| signal.to_string());
    format!("killed by {name}")
}

/// Exit code of `hypha launch unit`.
///
/// 0 for a node that exited with success or was stopped on request, the exit
/// code of the node otherwise, and 1 for a node that a signal ended.
#[must_use]
pub fn exit_code_for(status: ExitStatus, stopped_on_request: bool) -> u8 {
    let clean_end = stopped_on_request || status.success();
    if clean_end {
        return 0;
    }
    status.code().and_then(|code| u8::try_from(code).ok()).unwrap_or(1)
}

/// Runs `config` until its restart policy ends it or a stop signal arrives.
///
/// Returns the exit code for `hypha launch unit`.
///
/// # Errors
///
/// Returns an error when the restart delay is out of range, the signal
/// handlers or a helper thread cannot start, or the node cannot be waited for.
pub fn supervise_unit(config: &UnitConfig) -> Result<u8, String> {
    let first_delay = Duration::try_from_secs_f64(config.restart_delay_seconds)
        .map_err(|error| format!("unit '{}': restartDelaySeconds: {error}", config.name))?;
    let mut budget = RestartBudget::new(first_delay);
    let mut rejoin_after: Option<i64> = None;
    let (sender, receiver) = mpsc::channel::<SupervisorEvent>();
    start_signal_forwarder(sender.clone())?;

    loop {
        let started = Instant::now();
        let status = match run_node_once(config, rejoin_after, &sender, &receiver)? {
            RunEnd::Stopped(status) => {
                say!(
                    "[hypha launch] '{}' {} after a stop request",
                    config.name,
                    describe_exit(status)
                );
                return Ok(exit_code_for(status, true));
            }
            RunEnd::Exited(status) => status,
        };
        budget.observe_run(started.elapsed());
        rejoin_after = None;
        match restart_decision(config.restart, status) {
            RestartDecision::Stop => {
                say!("[hypha launch] '{}' {}; not restarting", config.name, describe_exit(status));
                return Ok(exit_code_for(status, false));
            }
            RestartDecision::RestartNow => {
                rejoin_after = Some(hypha_node::sdk::clock::steady_now_nanos());
                say!(
                    "[hypha launch] '{}' {} (simulated clock went back); restarting now",
                    config.name,
                    describe_exit(status)
                );
                if stop_requested_during(Duration::ZERO, &receiver)? {
                    say!("[hypha launch] '{}' stopped before its restart", config.name);
                    return Ok(0);
                }
                continue;
            }
            RestartDecision::RestartAfterDelay => {}
        }
        if budget.exhausted() {
            say!(
                "[hypha launch] '{}' gave up after {RESTART_BUDGET} restarts with no {} s run",
                config.name,
                LONG_RUN.as_secs()
            );
            return Ok(exit_code_for(status, false));
        }
        let (delay, attempt) = budget.next_restart();
        say!(
            "[hypha launch] '{}' {}; restarting in {:.1} s (attempt {attempt})",
            config.name,
            describe_exit(status),
            delay.as_secs_f64()
        );
        if stop_requested_during(delay, &receiver)? {
            say!("[hypha launch] '{}' stopped before its restart", config.name);
            return Ok(0);
        }
    }
}

fn run_node_once(
    config: &UnitConfig,
    rejoin_after: Option<i64>,
    sender: &Sender<SupervisorEvent>,
    receiver: &Receiver<SupervisorEvent>,
) -> Result<RunEnd, String> {
    let child = match spawn_node(config, rejoin_after) {
        Ok(child) => child,
        Err(error) => {
            say!("[hypha launch] failed to spawn '{}' ({}): {error}", config.name, config.exec);
            return Ok(RunEnd::Exited(ExitStatus::from_raw(SPAWN_FAILURE_WAIT_STATUS)));
        }
    };
    let raw_pid = i32::try_from(child.id())
        .map_err(|error| format!("pid of '{}' is out of range: {error}", config.name))?;
    let group = Pid::from_raw(raw_pid);
    say!("[hypha launch] started '{}' (pid {group})", config.name);
    #[cfg_attr(
        not(target_os = "linux"),
        expect(unused_mut, reason = "only the Linux main thread waits on the child")
    )]
    let mut node = watch_node(config, child, group, sender)?;

    let mut stopping = false;
    let mut kill_deadline: Option<Instant> = None;
    loop {
        let Some(event) = next_event(receiver, kill_deadline)? else {
            say!(
                "[hypha launch] '{}' did not stop in {} s; sending SIGKILL",
                config.name,
                STOP_GRACE_PERIOD.as_secs()
            );
            signal_group(&config.name, node.group, Signal::SIGKILL);
            kill_deadline = None;
            continue;
        };
        match event {
            #[cfg(target_os = "linux")]
            SupervisorEvent::NodeEnded => {
                let status = node
                    .child
                    .wait()
                    .map_err(|error| format!("cannot wait for '{}': {error}", config.name))?;
                return Ok(run_end(status, stopping));
            }
            #[cfg(not(target_os = "linux"))]
            SupervisorEvent::NodeExited(status) => return Ok(run_end(status, stopping)),
            SupervisorEvent::NodeWaitFailed(message) => {
                return Err(format!("cannot wait for '{}': {message}", config.name));
            }
            SupervisorEvent::StopRequested(_) if stopping => {}
            SupervisorEvent::StopRequested(signal_number) => {
                // Signal first: the window may already be gone (SIGHUP from tmux).
                signal_group(&config.name, node.group, Signal::SIGTERM);
                say!(
                    "[hypha launch] '{}': received signal {signal_number}; sending SIGTERM",
                    config.name
                );
                stopping = true;
                kill_deadline = Some(deadline_after(STOP_GRACE_PERIOD)?);
            }
        }
    }
}

fn run_end(status: ExitStatus, stopping: bool) -> RunEnd {
    if stopping {
        return RunEnd::Stopped(status);
    }
    RunEnd::Exited(status)
}

fn spawn_node(config: &UnitConfig, rejoin_after: Option<i64>) -> std::io::Result<Child> {
    let mut command = Command::new(&config.exec);
    command.args(&config.args);
    command.envs(&config.env);
    match rejoin_after {
        Some(nanos) => command.env(REJOIN_VARIABLE, nanos.to_string()),
        None => command.env_remove(REJOIN_VARIABLE),
    };
    if !config.working_directory.is_empty() {
        command.current_dir(&config.working_directory);
    }
    // Its own group: a Ctrl-C in the window reaches the supervisor only.
    command.process_group(0);
    // The node gets SIGTERM when the supervisor dies, and does not run when the
    // supervisor died before the node could ask for that.
    #[cfg(target_os = "linux")]
    hypha_node::sdk::sys::set_parent_death_signal(&mut command, nix::libc::SIGTERM)?;
    command.spawn()
}

/// Start watching `child` for its exit. On Linux a pidfd thread reports the exit
/// without reaping, so the main thread keeps the child; elsewhere the waiter
/// thread reaps it.
#[cfg(target_os = "linux")]
fn watch_node(
    config: &UnitConfig,
    child: Child,
    group: Pid,
    sender: &Sender<SupervisorEvent>,
) -> Result<RunningNode, String> {
    let mut child = child;
    let Some(rustix_pid) = rustix::process::Pid::from_raw(group.as_raw()) else {
        return Err(format!("pid of '{}' is not a process id", config.name));
    };
    let pidfd = match rustix::process::pidfd_open(rustix_pid, rustix::process::PidfdFlags::empty())
    {
        Ok(pidfd) => pidfd,
        Err(error) => {
            // The node must not run unwatched: stop it, then refuse the unit.
            signal_group(&config.name, group, Signal::SIGKILL);
            let _reaped = child.wait();
            return Err(format!("cannot open a handle on '{}': {error}", config.name));
        }
    };
    start_exit_watcher(pidfd, sender.clone())?;
    Ok(RunningNode { group, child })
}

#[cfg(not(target_os = "linux"))]
fn watch_node(
    _config: &UnitConfig,
    child: Child,
    group: Pid,
    sender: &Sender<SupervisorEvent>,
) -> Result<RunningNode, String> {
    start_exit_waiter(child, sender.clone())?;
    Ok(RunningNode { group })
}

/// Blocks for the next event. Returns `None` when `deadline` passes first.
fn next_event(
    receiver: &Receiver<SupervisorEvent>,
    deadline: Option<Instant>,
) -> Result<Option<SupervisorEvent>, String> {
    let Some(deadline) = deadline else {
        return receiver.recv().map(Some).map_err(|_| CHANNEL_CLOSED.to_string());
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    match receiver.recv_timeout(remaining) {
        Ok(event) => Ok(Some(event)),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => Err(CHANNEL_CLOSED.to_string()),
    }
}

fn stop_requested_during(
    delay: Duration,
    receiver: &Receiver<SupervisorEvent>,
) -> Result<bool, String> {
    let deadline = deadline_after(delay)?;
    loop {
        match next_event(receiver, Some(deadline))? {
            None => return Ok(false),
            Some(SupervisorEvent::StopRequested(_)) => return Ok(true),
            Some(_) => {}
        }
    }
}

fn deadline_after(duration: Duration) -> Result<Instant, String> {
    Instant::now()
        .checked_add(duration)
        .ok_or_else(|| format!("a wait of {duration:?} is out of range"))
}

/// Signal the node's process group. On Linux the leader is not reaped yet, so the
/// group id cannot name another group.
fn signal_group(unit_name: &str, group: Pid, signal: Signal) {
    match killpg(group, signal) {
        // ESRCH: the whole group already exited.
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(error) => say!("[hypha launch] cannot send {signal} to '{unit_name}': {error}"),
    }
}

/// Reports the exit of the node whose pidfd it holds, without reaping it.
#[cfg(target_os = "linux")]
struct ExitWatcher {
    pidfd: std::os::fd::OwnedFd,
    sender: Sender<SupervisorEvent>,
}

#[cfg(target_os = "linux")]
impl ExitWatcher {
    fn run(self) {
        let event = loop {
            let mut fds = [rustix::event::PollFd::new(&self.pidfd, rustix::event::PollFlags::IN)];
            match rustix::event::poll(&mut fds, None) {
                Ok(_) => break SupervisorEvent::NodeEnded,
                Err(rustix::io::Errno::INTR) => {}
                Err(error) => break SupervisorEvent::NodeWaitFailed(error.to_string()),
            }
        };
        // A send fails only after the supervisor returned; nobody waits for the event then.
        let _unreceived = self.sender.send(event);
    }
}

#[cfg(target_os = "linux")]
fn start_exit_watcher(
    pidfd: std::os::fd::OwnedFd,
    sender: Sender<SupervisorEvent>,
) -> Result<(), String> {
    let watcher = ExitWatcher { pidfd, sender };
    spawn_thread("wait", "exit watcher", move || watcher.run())
}

#[cfg(not(target_os = "linux"))]
struct ExitWaiter {
    child: Child,
    sender: Sender<SupervisorEvent>,
}

#[cfg(not(target_os = "linux"))]
impl ExitWaiter {
    fn run(mut self) {
        let event = match self.child.wait() {
            Ok(status) => SupervisorEvent::NodeExited(status),
            Err(error) => SupervisorEvent::NodeWaitFailed(error.to_string()),
        };
        // A send fails only after the supervisor returned; nobody waits for the event then.
        let _unreceived = self.sender.send(event);
    }
}

#[cfg(not(target_os = "linux"))]
fn start_exit_waiter(child: Child, sender: Sender<SupervisorEvent>) -> Result<(), String> {
    let waiter = ExitWaiter { child, sender };
    spawn_thread("wait", "exit waiter", move || waiter.run())
}

struct SignalForwarder {
    signals: Signals,
    sender: Sender<SupervisorEvent>,
}

impl SignalForwarder {
    fn run(mut self) {
        for signal_number in self.signals.forever() {
            if self.sender.send(SupervisorEvent::StopRequested(signal_number)).is_err() {
                return;
            }
        }
    }
}

fn start_signal_forwarder(sender: Sender<SupervisorEvent>) -> Result<(), String> {
    let signals = Signals::new([SIGINT, SIGTERM, SIGHUP])
        .map_err(|error| format!("cannot install signal handlers: {error}"))?;
    let forwarder = SignalForwarder { signals, sender };
    spawn_thread("signals", "signal", move || forwarder.run())
}

#[cfg(test)]
mod budget_tests {
    use super::{LONG_RUN, RESTART_BUDGET, RestartBudget};
    use std::time::Duration;

    fn delays(first: Duration, count: usize) -> Vec<Duration> {
        let mut budget = RestartBudget::new(first);
        let mut delays = Vec::new();
        for _ in 0..count {
            delays.push(budget.next_restart().0);
        }
        delays
    }

    #[test]
    fn the_delay_doubles_and_stops_at_thirty_seconds() {
        let secs: Vec<u64> =
            delays(Duration::from_secs(1), 7).iter().map(Duration::as_secs).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 16, 30, 30]);
        assert!(delays(Duration::ZERO, 5).iter().all(Duration::is_zero));
        let above_cap = Duration::from_secs(45);
        assert!(delays(above_cap, 3).iter().all(|delay| *delay == above_cap));
    }

    #[test]
    fn a_long_run_resets_the_delay_and_the_count() {
        let mut budget = RestartBudget::new(Duration::from_secs(1));
        for _ in 0..3 {
            let _restart = budget.next_restart();
        }
        budget.observe_run(LONG_RUN.saturating_sub(Duration::from_millis(1)));
        assert_eq!(budget.next_restart(), (Duration::from_secs(8), 4), "59.999 s is short");
        budget.observe_run(LONG_RUN);
        assert_eq!(budget.next_restart(), (Duration::from_secs(1), 1));
    }

    #[test]
    fn the_budget_is_exhausted_after_ten_restarts() {
        let mut budget = RestartBudget::new(Duration::ZERO);
        for _ in 0..RESTART_BUDGET {
            assert!(!budget.exhausted());
            let _restart = budget.next_restart();
        }
        assert!(budget.exhausted());
    }
}
