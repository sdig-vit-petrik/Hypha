//! Tests for the `hypha launch` command line.

#![allow(clippy::unwrap_used)]

use hypha_tools::launch_cli::{LaunchCommand, parse_launch_arguments};
use std::path::PathBuf;
use std::process::Command;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_hypha");

fn parse(args: &[&str]) -> Result<Option<LaunchCommand>, String> {
    let owned: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    parse_launch_arguments(&owned)
}

#[test]
fn up_with_all_options() {
    let command = parse(&["up", "fleet.toml", "--session", "bench", "--attach"]).unwrap();
    assert_eq!(
        command,
        Some(LaunchCommand::Up {
            units_file: PathBuf::from("fleet.toml"),
            session: Some("bench".to_string()),
            attach: true
        })
    );
}

#[test]
fn options_may_come_before_the_file() {
    let command = parse(&["down", "--session", "bench", "fleet.toml"]).unwrap();
    assert_eq!(
        command,
        Some(LaunchCommand::Down {
            units_file: PathBuf::from("fleet.toml"),
            session: Some("bench".to_string())
        })
    );
}

#[test]
fn attach_and_unit_and_help() {
    assert_eq!(
        parse(&["attach", "fleet.toml"]).unwrap(),
        Some(LaunchCommand::Attach { units_file: PathBuf::from("fleet.toml"), session: None })
    );
    assert_eq!(
        parse(&["unit", "fleet.toml", "planner"]).unwrap(),
        Some(LaunchCommand::Unit {
            units_file: PathBuf::from("fleet.toml"),
            unit_name: "planner".to_string()
        })
    );
    assert_eq!(parse(&["--help"]).unwrap(), None);
    assert_eq!(parse(&["-h"]).unwrap(), None);
}

#[test]
fn bad_arguments_are_refused() {
    let cases: [(&[&str], &str); 11] = [
        (&[], "Usage: hypha launch <COMMAND>"),
        (&["run", "fleet.toml"], "unrecognized subcommand 'run'"),
        (&["generate", "fleet.toml"], "unrecognized subcommand 'generate'"),
        (&["up"], "<UNITS_FILE>"),
        (&["up", "fleet.toml", "--verbose"], "unexpected argument '--verbose'"),
        (&["up", "fleet.toml", "--session"], "a value is required for '--session <SESSION>'"),
        (
            &["up", "fleet.toml", "--session", "--attach"],
            "a value is required for '--session <SESSION>'",
        ),
        (
            &["up", "fleet.toml", "--session", "a", "--session", "b"],
            "cannot be used multiple times",
        ),
        (&["down", "fleet.toml", "--attach"], "unexpected argument '--attach'"),
        (&["up", "a.toml", "b.toml"], "unexpected argument 'b.toml'"),
        (&["unit", "fleet.toml"], "<UNIT_NAME>"),
    ];
    for (args, expected) in cases {
        let error = parse(args).unwrap_err();
        assert!(error.contains(expected), "{args:?}: '{error}' lacks '{expected}'");
    }
}

#[test]
fn binary_refuses_a_missing_units_file() {
    let output = Command::new(LAUNCHER)
        .arg("launch")
        .args(["up", "/nonexistent/hypha_fleet.toml"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot read '/nonexistent/hypha_fleet.toml'"), "{stderr}");
}

#[test]
fn binary_prints_usage_on_bad_arguments_and_help() {
    let bad = Command::new(LAUNCHER).arg("launch").output().unwrap();
    assert_eq!(bad.status.code(), Some(1));
    let bad_stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(bad_stderr.contains("Usage: hypha launch <COMMAND>"), "{bad_stderr}");

    let help = Command::new(LAUNCHER).arg("launch").arg("--help").output().unwrap();
    assert_eq!(help.status.code(), Some(0));
    let help_stdout = String::from_utf8_lossy(&help.stdout);
    assert!(help_stdout.contains("Usage: hypha launch <COMMAND>"), "{help_stdout}");
    assert!(help_stdout.contains("--attach"), "{help_stdout}");
}
