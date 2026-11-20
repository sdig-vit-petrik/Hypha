//! Tests for the one-unit supervisor behind `hypha launch unit`.

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
#![allow(clippy::indexing_slicing)]
#![allow(clippy::arithmetic_side_effects)]

use hypha_tools::launch_supervisor::{
    RestartDecision, describe_exit, exit_code_for, restart_decision,
};
use hypha_tools::launch_units::RestartPolicy;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::io::{BufRead, BufReader};
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_hypha");

fn exited(code: i32) -> ExitStatus {
    ExitStatus::from_raw(code * 256)
}

fn killed_by(signal: Signal) -> ExitStatus {
    ExitStatus::from_raw(signal as i32)
}

fn test_dir(test_name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("hypha_launch_supervisor_{}_{test_name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_units_file(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fleet.toml");
    std::fs::write(&path, body).unwrap();
    path
}

struct Finished {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_unit_to_end(path: &Path, unit: &str, limit: Duration) -> Finished {
    let child = Command::new(LAUNCHER)
        .arg("launch")
        .arg("unit")
        .arg(path)
        .arg(unit)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || sender.send(child.wait_with_output()).unwrap());
    let output = receiver.recv_timeout(limit).expect("hypha_launch unit did not exit").unwrap();
    Finished {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Starts `hypha launch unit`, waits for a stdout line that holds `marker`,
/// sends `signal` to the supervisor, and returns all it printed.
fn signal_after_marker(
    path: &Path,
    unit: &str,
    marker: &str,
    signal: Signal,
    limit: Duration,
) -> Finished {
    let mut child: Child = Command::new(LAUNCHER)
        .arg("launch")
        .arg("unit")
        .arg(path)
        .arg(unit)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (line_sender, line_receiver) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if line_sender.send(line).is_err() {
                return;
            }
        }
    });
    let mut printed = String::new();
    loop {
        let line = line_receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the marker line did not appear");
        let found = line.contains(marker);
        printed.push_str(&line);
        printed.push('\n');
        if found {
            break;
        }
    }
    kill(Pid::from_raw(i32::try_from(child.id()).unwrap()), signal).unwrap();
    let (status_sender, status_receiver) = mpsc::channel();
    thread::spawn(move || status_sender.send(child.wait()).unwrap());
    let status = status_receiver.recv_timeout(limit).expect("supervisor did not exit").unwrap();
    while let Ok(line) = line_receiver.recv_timeout(Duration::from_secs(2)) {
        printed.push_str(&line);
        printed.push('\n');
    }
    Finished { code: status.code(), stdout: printed, stderr: String::new() }
}

#[test]
fn should_restart_decisions_for_each_policy_and_code() {
    use RestartDecision::{RestartAfterDelay, RestartNow, Stop};
    use RestartPolicy::{Always, Never, OnFailure};
    let cases = [
        (Never, exited(0), Stop),
        (Never, exited(70), Stop),
        (Never, exited(75), RestartNow),
        (Never, exited(78), Stop),
        (Never, killed_by(Signal::SIGSEGV), Stop),
        (OnFailure, exited(0), Stop),
        (OnFailure, exited(70), RestartAfterDelay),
        (OnFailure, exited(75), RestartNow),
        (OnFailure, exited(78), Stop),
        (OnFailure, killed_by(Signal::SIGSEGV), RestartAfterDelay),
        (Always, exited(0), RestartAfterDelay),
        (Always, exited(70), RestartAfterDelay),
        (Always, exited(75), RestartNow),
        (Always, exited(78), Stop),
        (Always, killed_by(Signal::SIGSEGV), RestartAfterDelay),
    ];
    for (policy, status, expected) in cases {
        assert_eq!(restart_decision(policy, status), expected, "{policy:?} {status:?}");
    }
}

#[test]
fn exit_is_described_by_code_or_signal() {
    assert_eq!(describe_exit(exited(3)), "exited 3");
    assert_eq!(describe_exit(killed_by(Signal::SIGSEGV)), "killed by SIGSEGV");
}

#[test]
fn exit_code_mapping() {
    assert_eq!(exit_code_for(exited(0), false), 0);
    assert_eq!(exit_code_for(exited(3), false), 3);
    assert_eq!(exit_code_for(killed_by(Signal::SIGSEGV), false), 1);
    assert_eq!(exit_code_for(exited(3), true), 0);
    assert_eq!(exit_code_for(killed_by(Signal::SIGKILL), true), 0);
}

#[test]
fn node_exit_code_is_reported_without_restart() {
    let dir = test_dir("exit_code");
    let path = write_units_file(
        &dir,
        "[[units]]\nname = \"fails\"\nexec = \"/bin/sh\"\nargs = [\"-c\", \"exit 3\"]\n",
    );
    let finished = run_unit_to_end(&path, "fails", Duration::from_secs(10));
    assert_eq!(finished.code, Some(3));
    assert!(finished.stdout.contains("'fails' exited 3; not restarting"), "{}", finished.stdout);
}

#[test]
fn on_failure_restarts_until_success() {
    let dir = test_dir("on_failure");
    let body = format!(
        r#"[[units]]
name = "flaky"
exec = "/bin/sh"
args = ["-c", 'n=$(cat count 2>/dev/null || echo 0); n=$((n+1)); echo $n > count; [ "$n" -ge 3 ]']
workingDirectory = "{}"
restart = "on-failure"
restartDelaySeconds = 0.0
"#,
        dir.display()
    );
    let path = write_units_file(&dir, &body);
    let finished = run_unit_to_end(&path, "flaky", Duration::from_secs(10));
    assert_eq!(finished.code, Some(0), "{}", finished.stdout);
    assert_eq!(finished.stdout.matches("started 'flaky'").count(), 3, "{}", finished.stdout);
    assert!(finished.stdout.contains("(attempt 2)"), "{}", finished.stdout);
}

#[test]
fn environment_is_applied_on_every_run() {
    let dir = test_dir("environment");
    let body = r#"[[units]]
name = "env"
exec = "/bin/sh"
args = ["-c", 'echo "value=$HYPHA_TEST_VALUE"; [ -f seen ] && exit 0; touch seen; exit 1']
restart = "on-failure"
restartDelaySeconds = 0.0
workingDirectory = "DIR"

[units.env]
HYPHA_TEST_VALUE = "kept"
"#
    .replace("DIR", &dir.display().to_string());
    let path = write_units_file(&dir, &body);
    let finished = run_unit_to_end(&path, "env", Duration::from_secs(10));
    assert_eq!(finished.stdout.matches("value=kept").count(), 2, "{}", finished.stdout);
}

#[test]
fn spawn_failure_reports_127() {
    let dir = test_dir("spawn_failure");
    let path = write_units_file(
        &dir,
        "[[units]]\nname = \"ghost\"\nexec = \"/nonexistent/hypha_launch_test_binary\"\n",
    );
    let finished = run_unit_to_end(&path, "ghost", Duration::from_secs(10));
    assert_eq!(finished.code, Some(127));
    assert!(finished.stdout.contains("failed to spawn 'ghost'"), "{}", finished.stdout);
}

#[test]
fn unknown_unit_name_is_refused() {
    let dir = test_dir("unknown_unit");
    let path = write_units_file(&dir, "[[units]]\nname = \"a\"\nexec = \"/bin/true\"\n");
    let finished = run_unit_to_end(&path, "missing", Duration::from_secs(10));
    assert_eq!(finished.code, Some(1));
    assert!(finished.stderr.contains("no unit named 'missing'"), "{}", finished.stderr);
}

#[test]
fn stop_signals_stop_the_node_without_restart() {
    for (index, signal) in [Signal::SIGHUP, Signal::SIGTERM, Signal::SIGINT].iter().enumerate() {
        let dir = test_dir(&format!("stop_signal_{index}"));
        let path = write_units_file(
            &dir,
            "[[units]]\nname = \"sleeper\"\nexec = \"/bin/sleep\"\nargs = [\"30\"]\nrestart = \"always\"\n",
        );
        let finished = signal_after_marker(
            &path,
            "sleeper",
            "started 'sleeper'",
            *signal,
            Duration::from_secs(10),
        );
        assert_eq!(finished.code, Some(0), "{signal}: {}", finished.stdout);
        assert!(finished.stdout.contains("sending SIGTERM"), "{signal}: {}", finished.stdout);
        assert!(finished.stdout.contains("after a stop request"), "{signal}: {}", finished.stdout);
        assert_eq!(finished.stdout.matches("started 'sleeper'").count(), 1);
    }
}

#[test]
fn node_that_ignores_sigterm_is_killed_after_grace_period() {
    let dir = test_dir("stubborn");
    let path = write_units_file(
        &dir,
        r#"[[units]]
name = "stubborn"
exec = "/bin/sh"
args = ["-c", 'trap "" TERM; echo stubborn-ready; while :; do sleep 0.1; done']
"#,
    );
    let finished = signal_after_marker(
        &path,
        "stubborn",
        "stubborn-ready",
        Signal::SIGTERM,
        Duration::from_secs(15),
    );
    assert_eq!(finished.code, Some(0), "{}", finished.stdout);
    assert!(finished.stdout.contains("sending SIGKILL"), "{}", finished.stdout);
}

#[test]
fn stop_during_restart_delay_ends_without_restart() {
    let dir = test_dir("stop_in_delay");
    let path = write_units_file(
        &dir,
        "[[units]]\nname = \"quick\"\nexec = \"/bin/sh\"\nargs = [\"-c\", \"exit 0\"]\nrestart = \"always\"\nrestartDelaySeconds = 30.0\n",
    );
    let finished = signal_after_marker(
        &path,
        "quick",
        "restarting in",
        Signal::SIGTERM,
        Duration::from_secs(10),
    );
    assert_eq!(finished.code, Some(0), "{}", finished.stdout);
    assert!(finished.stdout.contains("stopped before its restart"), "{}", finished.stdout);
    assert_eq!(finished.stdout.matches("started 'quick'").count(), 1);
}

/// A started `hypha launch unit` and the lines it prints.
#[cfg(target_os = "linux")]
struct Running {
    child: Child,
    lines: mpsc::Receiver<String>,
    printed: String,
}

#[cfg(target_os = "linux")]
impl Running {
    /// Start the supervisor in its own process group, so a test can signal the
    /// whole group as a terminal would without touching the test runner.
    fn start(path: &Path, unit: &str) -> Self {
        let mut child: Child = Command::new(LAUNCHER)
            .arg("launch")
            .arg("unit")
            .arg(path)
            .arg(unit)
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (line_sender, lines) = mpsc::channel::<String>();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if line_sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self { child, lines, printed: String::new() }
    }

    fn pid(&self) -> Pid {
        Pid::from_raw(i32::try_from(self.child.id()).unwrap())
    }

    /// The first printed line that holds `marker`.
    fn line_with(&mut self, marker: &str) -> String {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(10))
                .expect("the marker line did not appear");
            self.printed.push_str(&line);
            self.printed.push('\n');
            if line.contains(marker) {
                return line;
            }
        }
    }

    fn wait(mut self, limit: Duration) -> Finished {
        let mut child = self.child;
        let (status_sender, status_receiver) = mpsc::channel();
        thread::spawn(move || status_sender.send(child.wait()).unwrap());
        let status = status_receiver.recv_timeout(limit).expect("supervisor did not exit").unwrap();
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(1)) {
            self.printed.push_str(&line);
            self.printed.push('\n');
        }
        Finished { code: status.code(), stdout: self.printed, stderr: String::new() }
    }
}

/// The number after `prefix` on a line such as `child 1234`.
#[cfg(target_os = "linux")]
fn number_after(line: &str, prefix: &str) -> i32 {
    let start = line.find(prefix).expect("the prefix is on the line") + prefix.len();
    let digits: String = line[start..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().unwrap()
}

/// True while `pid` names a live process; a zombie counts as gone.
#[cfg(target_os = "linux")]
fn alive(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let state = stat.rsplit(')').next().unwrap_or("").split_whitespace().next().unwrap_or("");
    state != "Z"
}

#[cfg(target_os = "linux")]
fn gone_within(pid: i32, limit: Duration) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < limit {
        if !alive(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

#[cfg(target_os = "linux")]
#[test]
fn the_node_runs_in_its_own_process_group() {
    let dir = test_dir("own_group");
    let path = write_units_file(
        &dir,
        r#"[[units]]
name = "grouped"
exec = "/bin/sh"
args = ["-c", 'echo "ids $$ $(cut -d" " -f5 /proc/$$/stat)"; sleep 30']
"#,
    );
    let mut running = Running::start(&path, "grouped");
    let line = running.line_with("ids ");
    let pid = number_after(&line, "ids ");
    let group = number_after(&line, &format!("ids {pid} "));
    assert_eq!(group, pid, "{line}");
    kill(running.pid(), Signal::SIGTERM).unwrap();
    let finished = running.wait(Duration::from_secs(10));
    assert_eq!(finished.code, Some(0), "{}", finished.stdout);
}

#[cfg(target_os = "linux")]
#[test]
fn a_stop_request_sends_one_term_to_the_node_group() {
    let dir = test_dir("one_term");
    let body = format!(
        r#"[[units]]
name = "counted"
exec = "/bin/sh"
workingDirectory = "{}"
args = ["-c", 'trap "echo sig >> seen" INT TERM; echo counted-ready; while [ ! -s seen ]; do sleep 0.05; done; sleep 0.5; exit 0']
"#,
        dir.display()
    );
    let path = write_units_file(&dir, &body);
    let mut running = Running::start(&path, "counted");
    running.line_with("counted-ready");
    // A terminal's Ctrl-C goes to its foreground group: the supervisor's.
    nix::sys::signal::killpg(running.pid(), Signal::SIGINT).unwrap();
    let finished = running.wait(Duration::from_secs(10));
    assert_eq!(finished.code, Some(0), "{}", finished.stdout);
    let seen = std::fs::read_to_string(dir.join("seen")).unwrap();
    assert_eq!(seen.lines().count(), 1, "the node got one signal: {seen:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn grandchild_in_the_node_group_is_stopped_with_it() {
    let dir = test_dir("grandchild");
    let path = write_units_file(
        &dir,
        r#"[[units]]
name = "parent"
exec = "/bin/sh"
args = ["-c", 'sleep 300 & echo "child $!"; wait']
"#,
    );
    let mut running = Running::start(&path, "parent");
    let line = running.line_with("child ");
    let grandchild = number_after(&line, "child ");
    kill(running.pid(), Signal::SIGTERM).unwrap();
    let finished = running.wait(Duration::from_secs(10));
    assert_eq!(finished.code, Some(0), "{}", finished.stdout);
    assert!(gone_within(grandchild, Duration::from_secs(6)), "the grandchild outlived its node");
}

#[cfg(target_os = "linux")]
#[test]
fn a_grandchild_that_ignores_sigterm_is_killed_with_its_group() {
    let dir = test_dir("stubborn_grandchild");
    let path = write_units_file(
        &dir,
        r#"[[units]]
name = "stubborn"
exec = "/bin/sh"
args = ["-c", 'trap "" TERM; sleep 300 & echo "child $!"; while :; do sleep 0.1; done']
"#,
    );
    let mut running = Running::start(&path, "stubborn");
    let line = running.line_with("child ");
    let grandchild = number_after(&line, "child ");
    kill(running.pid(), Signal::SIGTERM).unwrap();
    let finished = running.wait(Duration::from_secs(15));
    assert!(finished.stdout.contains("sending SIGKILL"), "{}", finished.stdout);
    assert!(gone_within(grandchild, Duration::from_secs(2)), "SIGKILL missed the grandchild");
}

#[cfg(target_os = "linux")]
#[test]
fn a_unit_ends_when_its_supervisor_is_killed() {
    let dir = test_dir("orphan");
    let path = write_units_file(
        &dir,
        "[[units]]\nname = \"sleeper\"\nexec = \"/bin/sleep\"\nargs = [\"60\"]\n",
    );
    let mut running = Running::start(&path, "sleeper");
    let line = running.line_with("started 'sleeper'");
    let node = number_after(&line, "(pid ");
    kill(running.pid(), Signal::SIGKILL).unwrap();
    let _finished = running.wait(Duration::from_secs(5));
    assert!(gone_within(node, Duration::from_secs(5)), "the node outlived its supervisor");
}

#[test]
fn eleventh_failure_gives_up_with_the_node_code() {
    let dir = test_dir("give_up");
    let path = write_units_file(
        &dir,
        "[[units]]\nname = \"x\"\nexec = \"/bin/sh\"\nargs = [\"-c\", \"exit 3\"]\nrestart = \"on-failure\"\nrestartDelaySeconds = 0.0\n",
    );
    let finished = run_unit_to_end(&path, "x", Duration::from_secs(20));
    assert_eq!(finished.code, Some(3), "{}", finished.stdout);
    assert_eq!(finished.stdout.matches("started 'x'").count(), 11, "{}", finished.stdout);
    assert!(finished.stdout.contains("restarting in 0.0 s (attempt 1)"), "{}", finished.stdout);
    assert!(finished.stdout.contains("(attempt 10)"), "{}", finished.stdout);
    assert!(!finished.stdout.contains("(attempt 11)"), "{}", finished.stdout);
    assert!(
        finished.stdout.contains("[hypha launch] 'x' gave up after 10 restarts with no 60 s run"),
        "{}",
        finished.stdout
    );
}

#[test]
fn a_spawn_failure_is_restarted_under_the_policy_and_then_given_up() {
    let dir = test_dir("spawn_give_up");
    let path = write_units_file(
        &dir,
        "[[units]]\nname = \"x\"\nexec = \"/nonexistent/hypha_node\"\nrestart = \"always\"\nrestartDelaySeconds = 0.0\n",
    );
    let finished = run_unit_to_end(&path, "x", Duration::from_secs(20));
    assert_eq!(finished.code, Some(127), "{}", finished.stdout);
    assert_eq!(finished.stdout.matches("failed to spawn 'x'").count(), 11, "{}", finished.stdout);
    assert!(finished.stdout.contains("gave up after 10 restarts"), "{}", finished.stdout);
}

#[test]
fn a_config_refusal_is_never_restarted() {
    let dir = test_dir("config_refusal");
    let path = write_units_file(
        &dir,
        "[[units]]\nname = \"x\"\nexec = \"/bin/sh\"\nargs = [\"-c\", \"exit 78\"]\nrestart = \"always\"\nrestartDelaySeconds = 0.0\n",
    );
    let finished = run_unit_to_end(&path, "x", Duration::from_secs(10));
    assert_eq!(finished.code, Some(78), "{}", finished.stdout);
    assert_eq!(finished.stdout.matches("started 'x'").count(), 1, "{}", finished.stdout);
    assert!(finished.stdout.contains("'x' exited 78; not restarting"), "{}", finished.stdout);
}

/// A unit that exits 75 until its third run, then 0, and records the rejoin
/// variable of each run.
fn rewinding_unit(dir: &Path, restart: &str, runs: u32) -> PathBuf {
    let script = format!(
        "n=$(cat count 2>/dev/null || echo 0); n=$((n+1)); echo $n > count; \
         echo \"run $n ${{HYPHA_REJOIN_AFTER_NANOS:-unset}}\" >> runs; \
         [ \"$n\" -ge {runs} ] && exit 0; exit 75"
    );
    let body = format!(
        "[[units]]\nname = \"x\"\nexec = \"/bin/sh\"\nworkingDirectory = \"{}\"\nargs = [\"-c\", '{script}']\nrestart = \"{restart}\"\nrestartDelaySeconds = 30.0\n",
        dir.display()
    );
    write_units_file(dir, &body)
}

#[test]
fn a_simulated_rewind_restarts_at_once_under_no_policy() {
    let dir = test_dir("rewind_now");
    let path = rewinding_unit(&dir, "no", 3);
    let started = std::time::Instant::now();
    let finished = run_unit_to_end(&path, "x", Duration::from_secs(10));
    assert!(started.elapsed() < Duration::from_secs(5), "a rewind waits for no delay");
    assert_eq!(finished.code, Some(0), "{}", finished.stdout);
    assert_eq!(finished.stdout.matches("started 'x'").count(), 3, "{}", finished.stdout);
    let line = "'x' exited 75 (simulated clock went back); restarting now";
    assert_eq!(finished.stdout.matches(line).count(), 2, "{}", finished.stdout);
    let runs = std::fs::read_to_string(dir.join("runs")).unwrap();
    let markers: Vec<&str> = runs.lines().map(|run| run.rsplit(' ').next().unwrap()).collect();
    assert_eq!(markers.first(), Some(&"unset"), "the first start has no rejoin: {runs}");
    for marker in markers.iter().skip(1) {
        let nanos: u64 = marker.parse().expect("a restart after 75 carries a steady time");
        assert!(nanos > 0, "{runs}");
    }
}

#[test]
fn a_rewind_does_not_count_toward_the_give_up() {
    let dir = test_dir("rewind_uncounted");
    let path = rewinding_unit(&dir, "on-failure", 13);
    let finished = run_unit_to_end(&path, "x", Duration::from_secs(15));
    assert_eq!(finished.code, Some(0), "{}", finished.stdout);
    assert_eq!(finished.stdout.matches("started 'x'").count(), 13, "{}", finished.stdout);
    assert!(!finished.stdout.contains("gave up"), "{}", finished.stdout);
}
