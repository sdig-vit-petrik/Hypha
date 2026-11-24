//! Tests that build, inspect and stop real tmux sessions on a private server (tmux 3.0 or newer).

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
#![allow(clippy::indexing_slicing)]
#![allow(clippy::arithmetic_side_effects)]

use hypha_tools::launch_session::tmux_args;
use hypha_tools::launch_tmux::{Tmux, session_attach, session_down, session_up};
use hypha_tools::launch_units::parse_units_file;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_hypha");

/// A tmux server private to one test. Dropping it stops the server.
struct PrivateServer {
    socket_name: String,
    tmux: Tmux,
}

impl PrivateServer {
    fn new(test_name: &str) -> Self {
        let socket_name = format!("hypha-test-{}-{test_name}", std::process::id());
        Self { tmux: Tmux::with_socket_name(&socket_name), socket_name }
    }

    fn run(&self, args: &[&str]) -> String {
        self.tmux.run(&tmux_args(args)).unwrap()
    }

    fn window_names(&self, session: &str) -> Vec<String> {
        let target = format!("={session}");
        self.run(&["list-windows", "-t", &target, "-F", "#{window_name}"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn scrollback(&self, session: &str, window: &str) -> String {
        let target = format!("={session}:={window}");
        self.run(&["capture-pane", "-p", "-J", "-S", "-", "-t", &target])
    }

    fn pane_field(&self, session: &str, window: &str, format: &str) -> String {
        let target = format!("={session}:={window}");
        self.run(&["list-panes", "-t", &target, "-F", format]).trim().to_string()
    }
}

impl Drop for PrivateServer {
    fn drop(&mut self) {
        // A test that ran down already stopped the server; that failure is expected.
        let _ = Command::new("tmux").args(["-L", &self.socket_name, "kill-server"]).output();
        // tmux leaves the socket file after the server exits.
        let _ = std::fs::remove_file(socket_path(&self.socket_name));
    }
}

fn socket_path(socket_name: &str) -> PathBuf {
    let user_id_output = Command::new("id").arg("-u").output().unwrap();
    let user_id = String::from_utf8_lossy(&user_id_output.stdout).trim().to_string();
    let base = std::env::var_os("TMUX_TMPDIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    base.join(format!("tmux-{user_id}")).join(socket_name)
}

fn write_units_file(test_name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("hypha_launch_lifecycle_{}_{test_name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fleet.toml");
    std::fs::write(&path, body).unwrap();
    path
}

fn up(server: &PrivateServer, path: &Path, session: &str) -> Result<(), String> {
    let parsed = parse_units_file(path).unwrap();
    session_up(&server.tmux, &parsed, session, Path::new(LAUNCHER), path)
}

fn wait_until(limit: Duration, what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

fn process_is_alive(pid: i32) -> bool {
    kill(Pid::from_raw(pid), None).is_ok()
}

const SLEEPERS: &str = r#"
[[units]]
name = "second"
exec = "/bin/sh"
args = ["-c", 'echo second-ready; sleep 30']
after = ["first"]

[[units]]
name = "first"
exec = "/bin/sh"
args = ["-c", 'echo first-ready; sleep 30']
"#;

#[test]
fn up_creates_one_window_per_unit_and_down_removes_the_session() {
    let server = PrivateServer::new("up_down");
    let path = write_units_file("up_down", SLEEPERS);
    up(&server, &path, "fleet").unwrap();
    assert_eq!(server.window_names("fleet"), vec!["first", "second"]);
    wait_until(Duration::from_secs(10), "second-ready", || {
        server.scrollback("fleet", "second").contains("second-ready")
    });
    session_down(&server.tmux, "fleet", Duration::from_secs(15)).unwrap();
    assert!(!server.tmux.session_exists("fleet").unwrap());
}

#[test]
fn second_up_is_refused() {
    let server = PrivateServer::new("second_up");
    let path = write_units_file("second_up", SLEEPERS);
    up(&server, &path, "fleet").unwrap();
    let error = up(&server, &path, "fleet").unwrap_err();
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(server.window_names("fleet"), vec!["first", "second"]);
}

#[test]
fn history_limit_reaches_unit_windows() {
    let server = PrivateServer::new("history");
    let body = format!("[session]\nhistoryLimit = 2500\n{SLEEPERS}");
    let path = write_units_file("history", &body);
    up(&server, &path, "fleet").unwrap();
    assert_eq!(server.pane_field("fleet", "first", "#{history_limit}"), "2500");
}

#[test]
fn crashing_unit_restarts_and_scrollback_keeps_both_runs() {
    let server = PrivateServer::new("crash");
    let path = write_units_file(
        "crash",
        r#"
[[units]]
name = "crasher"
exec = "/bin/sh"
args = ["-c", 'echo run-marker; sleep 0.3; exit 1']
restart = "on-failure"
restartDelaySeconds = 0.2
"#,
    );
    up(&server, &path, "fleet").unwrap();
    wait_until(Duration::from_secs(10), "two runs", || {
        server.scrollback("fleet", "crasher").matches("run-marker").count() >= 2
    });
    let scrollback = server.scrollback("fleet", "crasher");
    assert!(scrollback.contains("exited 1; restarting in 0.2 s"), "{scrollback}");
}

#[test]
fn unit_that_ends_keeps_its_window() {
    let server = PrivateServer::new("ends");
    let path = write_units_file(
        "ends",
        "[[units]]\nname = \"done\"\nexec = \"/bin/sh\"\nargs = [\"-c\", \"echo done-marker; exit 4\"]\n",
    );
    up(&server, &path, "fleet").unwrap();
    wait_until(Duration::from_secs(10), "dead pane", || {
        server.pane_field("fleet", "done", "#{pane_dead}") == "1"
    });
    assert_eq!(server.window_names("fleet"), vec!["done"]);
    assert_eq!(server.pane_field("fleet", "done", "#{pane_dead_status}"), "4");
    let scrollback = server.scrollback("fleet", "done");
    assert!(scrollback.contains("done-marker"), "{scrollback}");
    assert!(scrollback.contains("not restarting"), "{scrollback}");
}

#[test]
fn down_waits_for_a_unit_that_ignores_sigterm() {
    let server = PrivateServer::new("stubborn_unit");
    let path = write_units_file(
        "stubborn_unit",
        r#"
[[units]]
name = "stubborn"
exec = "/bin/sh"
args = ["-c", 'trap "" TERM; echo stubborn-ready; while :; do sleep 0.1; done']
"#,
    );
    up(&server, &path, "fleet").unwrap();
    wait_until(Duration::from_secs(10), "stubborn-ready", || {
        server.scrollback("fleet", "stubborn").contains("stubborn-ready")
    });
    let supervisor_pid: i32 =
        server.pane_field("fleet", "stubborn", "#{pane_pid}").parse().unwrap();
    session_down(&server.tmux, "fleet", Duration::from_secs(15)).unwrap();
    assert!(!process_is_alive(supervisor_pid));
    assert!(!server.tmux.session_exists("fleet").unwrap());
}

#[test]
fn down_kills_the_process_group_after_the_wait_limit() {
    let server = PrivateServer::new("wait_limit");
    let path = write_units_file("wait_limit", SLEEPERS);
    up(&server, &path, "fleet").unwrap();
    server.run(&[
        "new-window",
        "-d",
        "-t",
        "=fleet:",
        "-n",
        "rogue",
        "sh",
        "-c",
        "trap '' TERM; while :; do sleep 0.1; done",
    ]);
    let rogue_pid: i32 = server.pane_field("fleet", "rogue", "#{pane_pid}").parse().unwrap();
    session_down(&server.tmux, "fleet", Duration::from_secs(1)).unwrap();
    wait_until(Duration::from_secs(5), "rogue process to end", || !process_is_alive(rogue_pid));
    assert!(!server.tmux.session_exists("fleet").unwrap());
}

#[test]
fn down_and_attach_on_a_missing_session_are_refused() {
    let server = PrivateServer::new("missing");
    let down_error = session_down(&server.tmux, "absent", Duration::from_secs(1)).unwrap_err();
    assert!(down_error.contains("no session 'absent'"), "{down_error}");
    let attach_error = session_attach(&server.tmux, "absent").unwrap_err();
    assert!(attach_error.contains("no session 'absent'"), "{attach_error}");
}

#[test]
fn installed_tmux_is_supported() {
    let server = PrivateServer::new("version");
    server.tmux.require_supported_version().unwrap();
}

#[test]
fn four_immediate_exits_keep_their_windows_without_pty_respawn() {
    let server = PrivateServer::new("four_fast_units");
    let names = ["position", "horizon", "control", "manager"];
    let body = names.iter().fold(String::new(), |mut acc, name| {
        use std::fmt::Write as _;
        let _ = writeln!(acc, "[[units]]\nname = \"{name}\"\nexec = \"/usr/bin/true\"");
        acc
    });
    let path = write_units_file("four_fast_units", &body);
    up(&server, &path, "fleet").unwrap();
    assert_eq!(server.window_names("fleet"), names);
    for name in names {
        wait_until(Duration::from_secs(10), "immediate exit", || {
            server.pane_field("fleet", name, "#{pane_dead}") == "1"
        });
        assert_eq!(server.pane_field("fleet", name, "#{pane_dead_status}"), "0");
    }
    session_down(&server.tmux, "fleet", Duration::from_secs(2)).unwrap();
}

/// A server that another session started holds that session's environment. `up` still
/// gives the units the caller's environment.
#[test]
fn units_get_the_callers_environment_when_the_server_already_runs() {
    let server = PrivateServer::new("caller_env");
    let mut start = Command::new("tmux");
    start.args(["-L", &server.socket_name, "new-session", "-d", "-s", "other", "cat"]).env_clear();
    for name in ["PATH", "HOME", "TMUX_TMPDIR"] {
        if let Some(value) = std::env::var_os(name) {
            start.env(name, value);
        }
    }
    assert!(start.status().unwrap().success(), "the earlier session starts");
    // Cargo sets this for the test process; the server above never had it.
    let expected = std::env::var("CARGO_MANIFEST_DIR").expect("cargo runs the test");
    let path = write_units_file(
        "caller_env",
        "[[units]]\nname = \"env\"\nexec = \"/bin/sh\"\nargs = [\"-c\", 'echo \"mark=$CARGO_MANIFEST_DIR=\"; sleep 30']\n",
    );
    up(&server, &path, "fleet").unwrap();
    wait_until(Duration::from_secs(10), "the unit's mark", || {
        server.scrollback("fleet", "env").contains("mark=")
    });
    let scrollback = server.scrollback("fleet", "env");
    assert!(scrollback.contains(&format!("mark={expected}=")), "{scrollback}");
}

/// A unit runs in a background process group of its window, so `hypha play` there cannot
/// read the terminal. It takes no keys and draws no progress line, and a key typed in the
/// window does not stop the replay: the player reaches the end of the recording (#109).
#[test]
fn a_key_typed_in_a_player_units_window_does_not_stall_the_replay() {
    let server = PrivateServer::new("player_key");
    let recording = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/sensor_pipeline/fixture/sensor_inputs.mcap"
    );
    let domain = format!("t_{}_player_key", std::process::id());
    let path = write_units_file(
        "player_key",
        &format!(
            "[[units]]\nname = \"player\"\nexec = \"{LAUNCHER}\"\nargs = [\"play\", \"--input\", \"{recording}\"]\n[units.env]\nHYPHA_DOMAIN = \"{domain}\"\n"
        ),
    );
    up(&server, &path, "fleet").unwrap();
    wait_until(Duration::from_secs(20), "the replay's start", || {
        server.scrollback("fleet", "player").contains("Playing back")
    });
    server.run(&["send-keys", "-t", "=fleet:=player", " "]);
    wait_until(Duration::from_secs(60), "the end of the recording", || {
        server.pane_field("fleet", "player", "#{pane_dead}") == "1"
    });
    assert_eq!(server.pane_field("fleet", "player", "#{pane_dead_status}"), "0");
    let scrollback = server.scrollback("fleet", "player");
    assert!(!scrollback.contains("Controls:"), "the player took the terminal: {scrollback}");
    assert!(!scrollback.contains("[PLAYING]"), "the player drew a progress line: {scrollback}");
}
