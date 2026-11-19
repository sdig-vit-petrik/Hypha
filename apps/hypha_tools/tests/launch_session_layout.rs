//! Tests for the tmux commands that `hypha launch up` runs.

#![allow(clippy::unwrap_used)]
#![allow(clippy::indexing_slicing)]

use hypha_tools::launch_session::{
    SETUP_WINDOW_NAME, session_name, session_theme, tmux_args, up_commands, window_theme,
};
use hypha_tools::launch_units::{ParsedUnits, parse_units_file};
use std::path::{Path, PathBuf};

fn parse(test_name: &str, body: &str) -> ParsedUnits {
    let path: PathBuf = std::env::temp_dir()
        .join(format!("hypha_launch_layout_{}_{test_name}.toml", std::process::id()));
    std::fs::write(&path, body).unwrap();
    let parsed = parse_units_file(&path).unwrap();
    let _ = std::fs::remove_file(path);
    parsed
}

const TWO_UNITS: &str = r#"
[session]
historyLimit = 2500

[[units]]
name = "planner"
exec = "/bin/planner"
after = ["lidar"]

[[units]]
name = "lidar"
exec = "/bin/lidar"
"#;

#[test]
fn up_commands_build_windows_in_start_order() {
    let parsed = parse("start_order", TWO_UNITS);
    let commands = up_commands(
        &parsed,
        "fleet",
        Path::new("/opt/bin/hypha"),
        Path::new("/etc/hypha/fleet.toml"),
        &[],
    )
    .unwrap();
    let mut expected = vec![
        tmux_args(&["new-session", "-d", "-s", "fleet", "-n", "hypha_launch_setup", "cat"]),
        tmux_args(&["set-option", "-t", "=fleet:", "history-limit", "2500"]),
    ];
    for (option, value) in session_theme() {
        expected.push(tmux_args(&["set-option", "-t", "=fleet:", option, &value]));
    }
    for name in ["lidar", "planner"] {
        let channel = format!("hypha_launch_{}_fleet_{name}", std::process::id());
        expected.push(tmux_args(&[
            "new-window",
            "-d",
            "-t",
            "=fleet:",
            "-n",
            name,
            "/bin/sh",
            "-c",
            r#"tmux wait-for "$1" || exit; shift; exec "$@""#,
            "hypha_launch_start",
            &channel,
            "/opt/bin/hypha",
            "launch",
            "unit",
            "/etc/hypha/fleet.toml",
            name,
        ]));
        expected.push(tmux_args(&[
            "set-option",
            "-w",
            "-t",
            &format!("=fleet:={name}"),
            "remain-on-exit",
            "on",
        ]));
        for (option, value) in window_theme() {
            expected.push(tmux_args(&[
                "set-option",
                "-w",
                "-t",
                &format!("=fleet:={name}"),
                option,
                &value,
            ]));
        }
        expected.push(tmux_args(&["wait-for", "-S", &channel]));
    }
    expected.push(tmux_args(&["kill-window", "-t", "=fleet:=hypha_launch_setup"]));
    assert_eq!(commands, expected);
}

#[test]
fn the_session_gets_the_given_environment_one_variable_per_command() {
    let parsed = parse("environment", "[[units]]\nname = \"a\"\nexec = \"/bin/a\"\n");
    let environment =
        [("PATH".to_string(), "/a b:/c".to_string()), ("X".to_string(), String::new())];
    let commands =
        up_commands(&parsed, "fleet", Path::new("/l"), Path::new("/f.toml"), &environment).unwrap();
    assert_eq!(
        commands[0],
        tmux_args(&["new-session", "-d", "-s", "fleet", "-n", "hypha_launch_setup", "cat"])
    );
    assert_eq!(commands[1], tmux_args(&["set-environment", "-t", "=fleet:", "PATH", "/a b:/c"]));
    assert_eq!(commands[2], tmux_args(&["set-environment", "-t", "=fleet:", "X", ""]));
}

/// tmux refuses a command over 16 KiB; a CI environment is larger than that in total.
#[test]
fn a_large_environment_stays_within_the_tmux_command_size() {
    let parsed = parse("large", "[[units]]\nname = \"a\"\nexec = \"/bin/a\"\n");
    let environment: Vec<(String, String)> =
        (0..100).map(|index| (format!("VAR_{index}"), "x".repeat(1000))).collect();
    let commands =
        up_commands(&parsed, "fleet", Path::new("/l"), Path::new("/f.toml"), &environment).unwrap();
    for command in &commands {
        let bytes: usize = command.iter().map(|arg| arg.len() + 1).sum();
        assert!(bytes < 8 * 1024, "a {bytes}-byte tmux command");
    }
}

#[test]
fn paths_with_spaces_stay_single_arguments() {
    let parsed = parse("spaces", "[[units]]\nname = \"a\"\nexec = \"/bin/a\"\n");
    let commands = up_commands(
        &parsed,
        "fleet",
        Path::new("/opt/my tools/hypha"),
        Path::new("/home/robot/my \"fleet\".toml"),
        &[],
    )
    .unwrap();
    let window = commands.iter().find(|command| command[0] == "new-window").unwrap();
    assert_eq!(
        window[11..],
        tmux_args(&["/opt/my tools/hypha", "launch", "unit", "/home/robot/my \"fleet\".toml", "a"])
    );
}

#[test]
fn default_history_limit_is_set() {
    let parsed = parse("default_history", "[[units]]\nname = \"a\"\nexec = \"/bin/a\"\n");
    let commands =
        up_commands(&parsed, "fleet", Path::new("/l"), Path::new("/f.toml"), &[]).unwrap();
    assert_eq!(commands[1], tmux_args(&["set-option", "-t", "=fleet:", "history-limit", "10000"]));
}

#[test]
fn setup_window_name_is_reserved() {
    let body = format!("[[units]]\nname = \"{SETUP_WINDOW_NAME}\"\nexec = \"/bin/a\"\n");
    let parsed = parse("reserved", &body);
    let error =
        up_commands(&parsed, "fleet", Path::new("/l"), Path::new("/f.toml"), &[]).unwrap_err();
    assert!(error.contains("reserved"), "{error}");
}

#[test]
fn session_name_comes_from_file_stem() {
    assert_eq!(session_name(Path::new("/etc/hypha/fleet.toml"), None).unwrap(), "fleet");
    assert_eq!(session_name(Path::new("robot_1.toml"), None).unwrap(), "robot_1");
}

#[test]
fn requested_session_name_replaces_the_stem() {
    let name = session_name(Path::new("/etc/hypha/fleet.toml"), Some("bench-2")).unwrap();
    assert_eq!(name, "bench-2");
}

#[test]
fn session_names_tmux_cannot_target_are_refused() {
    let from_stem = session_name(Path::new("/etc/hypha/fleet.v2.toml"), None).unwrap_err();
    assert!(from_stem.contains("--session"), "{from_stem}");
    for requested in ["a:b", "a.b", "", "a b"] {
        let path = Path::new("/etc/hypha/fleet.toml");
        assert!(session_name(path, Some(requested)).is_err(), "'{requested}' accepted");
    }
}
