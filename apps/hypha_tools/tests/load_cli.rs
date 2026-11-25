//! `hypha load` end to end: its exit codes and one small run, asserting counted facts
//! only, never a speed or a latency of the host.

#![expect(clippy::expect_used, reason = "a test fails on the first refused step")]

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const HYPHA: &str = env!("CARGO_BIN_EXE_hypha");

/// The longest a run of these tests may take before the test gives up on it: a guard
/// against a hang, never a measure of the host.
const RUN_GUARD: Duration = Duration::from_secs(150);

/// A directory of its own for one test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("hypha_load_{test}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create the scratch directory");
        Self(directory)
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).expect("write a file");
        path
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Two processes and one topic at a low rate, the recorder on or off; `limits` is the
/// body of `[limits]`. The cool-down of 1 s leaves the last messages of the window ample
/// time to arrive before the stop.
fn workload(duration_s: f64, recorder: bool, limits: &str) -> String {
    format!(
        r#"
[run]
duration_s = {duration_s}
warmup_s = 0.5
cooldown_s = 1.0
sample_interval_s = 0.5
bounded_memory = false

[recorder]
enabled = {recorder}

[[process]]
name = "talker"

[[process]]
name = "listener"

[[topic]]
name = "/load_cli/chatter"
bytes = 64
rate_hz = 20
publisher = "talker"
subscribers = [{{ process = "listener" }}]

[limits]
{limits}
"#
    )
}

fn load(arguments: &[&Path]) -> Output {
    Command::new(HYPHA).arg("load").args(arguments).output().expect("run hypha load")
}

/// The value at the JSON `pointer` of `value`, null when there is none.
fn at<'v>(value: &'v serde_json::Value, pointer: &str) -> &'v serde_json::Value {
    value.pointer(pointer).unwrap_or(&serde_json::Value::Null)
}

fn report_of(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path).expect("the report was written");
    serde_json::from_str(&text).expect("the report is JSON")
}

#[test]
fn help_exits_zero_and_names_the_exit_codes() {
    let output = Command::new(HYPHA).args(["load", "--help"]).output().expect("run");
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(text.contains("70"), "{text}");
    assert!(!text.contains("--worker"), "the worker mode is hidden: {text}");
}

#[test]
fn an_invalid_command_line_or_workload_is_a_failed_run() {
    let scratch = Scratch::new("invalid");
    let no_report = Command::new(HYPHA).args(["load", "w.toml"]).output().expect("run");
    assert_eq!(no_report.status.code(), Some(70), "a missing --report");

    let missing = load(&[&scratch.path("absent.toml"), Path::new("--report"), &scratch.path("r")]);
    assert_eq!(missing.status.code(), Some(70), "an unreadable workload");

    let invalid =
        scratch.file("invalid.toml", &workload(1.0, true, "").replace("bytes = 64", "bytes = 4"));
    let output = load(&[&invalid, Path::new("--report"), &scratch.path("r.json")]);
    assert_eq!(output.status.code(), Some(70));
    let error = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(error.contains("probe header"), "{error}");
    assert!(!scratch.path("r.json").exists(), "an invalid workload writes no report");
}

#[test]
fn a_low_rate_run_receives_every_message_it_sent_and_passes() {
    let scratch = Scratch::new("pass");
    let path = scratch.file(
        "pass.toml",
        &workload(
            1.0,
            true,
            "max_loss = 0\nloss_accounted = true\nmax_refused_publishes = 0\nmax_unrecorded = 0",
        ),
    );
    let report_path = scratch.path("report.json");
    let output = load(&[&path, Path::new("--report"), &report_path]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(output.status.code(), Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("hypha load: pass"), "{stdout}");

    let report = report_of(&report_path);
    assert_eq!(at(&report, "/schema"), "hypha.load.report/1");
    assert_eq!(at(&report, "/outcome"), "pass");
    assert_eq!(at(&report, "/host/os"), std::env::consts::OS);
    let published = at(&report, "/topics/0/publish");
    let sent = at(published, "/sent_window").as_u64().expect("sent in the window");
    assert!(sent > 0, "{published}");
    assert_eq!(at(published, "/refused"), 0);
    let received = at(&report, "/topics/0/subscribers/0/result");
    assert_eq!(at(received, "/received_window").as_u64(), Some(sent), "{received}");
    assert_eq!(at(received, "/missing_sequences"), 0);
    assert_eq!(at(received, "/harness_gaps"), 0);
    assert_eq!(at(received, "/latency/count").as_u64(), Some(sent));
    let total = at(published, "/sent_total").as_u64().expect("sent over the run");
    assert_eq!(at(&report, "/recorder/topics/0/recorded").as_u64(), Some(total));
    assert_eq!(at(&report, "/recorder/exit_code"), 0);
    let processes = at(&report, "/processes").as_array().map(Vec::len);
    assert_eq!(processes, Some(3), "two workers and the recorder");
    let points = at(&report, "/series/points").as_array();
    assert!(points.is_some_and(|points| !points.is_empty()));
    let limits = at(&report, "/limits").as_array().expect("the limits");
    assert_eq!(limits.len(), 4, "{limits:?}");
    assert!(limits.iter().all(|limit| at(limit, "/pass") == true), "{limits:?}");
    assert!(scratch.path("report.mcap").exists(), "the recording lies beside the report");
}

#[test]
fn a_limit_no_measurement_can_meet_exits_one() {
    // `max_unrecorded` with the recorder off: the value is never measured, and a limit
    // whose value was not measured fails, whatever the host does.
    let scratch = Scratch::new("limit");
    let path = scratch.file("limit.toml", &workload(1.0, false, "max_unrecorded = 0"));
    let report_path = scratch.path("report.json");
    let output = load(&[&path, Path::new("--report"), &report_path]);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("limit failed: max_unrecorded"), "{stdout}");
    assert_eq!(at(&report_of(&report_path), "/outcome"), "limit_failed");
}

#[test]
fn a_worker_that_dies_fails_the_run_with_seventy() {
    let scratch = Scratch::new("killed");
    // A long window, so that the worker dies inside the run; its death, not a timeout,
    // ends it.
    let path = scratch.file("killed.toml", &workload(120.0, true, ""));
    let report_path = scratch.path("report.json");
    let mut child = Command::new(HYPHA)
        .arg("load")
        .arg(&path)
        .arg("--report")
        .arg(&report_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start hypha load");
    let stdout = child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let pid = lines
        .by_ref()
        .map_while(Result::ok)
        .find_map(|line| {
            line.strip_prefix("hypha load: process listener runs as pid ")
                .and_then(|pid| pid.trim().parse::<i32>().ok())
        })
        .expect("the listener's pid");
    // The recorder starts once every worker is ready: from then on the run is under way.
    let recorder = lines.by_ref().map_while(Result::ok).find(|line| line.contains("recorder runs"));
    assert!(recorder.is_some(), "the run reached the recorder");
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), nix::sys::signal::Signal::SIGKILL)
        .expect("kill the listener by its pid");
    // The coordinator goes on writing; its output is read to the end beside the wait.
    let rest = std::thread::spawn(move || lines.map_while(Result::ok).collect::<Vec<String>>());

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        assert!(started.elapsed() < RUN_GUARD, "hypha load did not end");
        std::thread::sleep(Duration::from_millis(50));
    };
    let rest = rest.join().unwrap_or_default();
    assert_eq!(status.code(), Some(70), "{rest:?}");
    let report = report_of(&report_path);
    assert_eq!(at(&report, "/outcome"), "run_failed");
    let reason = at(&report, "/reason").as_str().unwrap_or_default();
    assert!(reason.contains("listener"), "{reason}");
}
