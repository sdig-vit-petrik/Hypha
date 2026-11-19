//! Tests for launch units file parsing, validation, and topological sorting.

#![allow(clippy::unwrap_used)]
#![allow(clippy::indexing_slicing)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_precision_loss)]

use hypha_tools::launch_units::{
    DEFAULT_HISTORY_LIMIT, RestartPolicy, is_valid_launch_name, parse_units_file,
};
use std::fs;

fn write_temp_toml(name: &str, content: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("{name}_{}.toml", std::process::id()));
    fs::write(&path, content).unwrap();
    path
}

#[test]
fn parses_basic_fields_and_defaults() {
    let path = write_temp_toml(
        "basic",
        r#"
[[units]]
name = "a"
exec = "/bin/a"
args = ["--flag", "value"]
workingDirectory = "/tmp"
restart = "on-failure"
restartDelaySeconds = 2.5

[units.env]
FOO = "bar"
"#,
    );

    let parsed = parse_units_file(&path).unwrap();
    assert_eq!(parsed.units.len(), 1);

    let unit = &parsed.units[0];
    assert_eq!(unit.name, "a");
    assert_eq!(unit.exec, "/bin/a");
    assert_eq!(unit.args, vec!["--flag", "value"]);
    assert_eq!(unit.env.get("FOO").map(String::as_str), Some("bar"));
    assert_eq!(unit.working_directory, "/tmp");
    assert_eq!(unit.restart, RestartPolicy::OnFailure);
    assert!((unit.restart_delay_seconds - 2.5).abs() < 1e-6);

    let _ = fs::remove_file(path);
}

#[test]
fn default_restart_is_never() {
    let path = write_temp_toml(
        "defaults",
        r#"
[[units]]
name = "a"
exec = "/bin/a"
"#,
    );

    let parsed = parse_units_file(&path).unwrap();
    assert_eq!(parsed.units[0].restart, RestartPolicy::Never);
    assert!((parsed.units[0].restart_delay_seconds - 1.0).abs() < 1e-6);

    let _ = fs::remove_file(path);
}

#[test]
fn orders_by_after_dependency() {
    let path = write_temp_toml(
        "order",
        r#"
[[units]]
name = "consumer"
exec = "/bin/consumer"
after = ["producer"]

[[units]]
name = "producer"
exec = "/bin/producer"
"#,
    );

    let parsed = parse_units_file(&path).unwrap();
    assert_eq!(parsed.start_order.len(), 2);

    let producer_pos = parsed.start_order.iter().position(|&x| x == 1).unwrap();
    let consumer_pos = parsed.start_order.iter().position(|&x| x == 0).unwrap();
    assert!(producer_pos < consumer_pos);

    let _ = fs::remove_file(path);
}

#[test]
fn rejects_undefined_after_reference() {
    let path = write_temp_toml(
        "bad_after",
        r#"
[[units]]
name = "a"
exec = "/bin/a"
after = ["does_not_exist"]
"#,
    );

    let err = parse_units_file(&path).unwrap_err();
    assert!(err.contains("does_not_exist"), "got: {err}");

    let _ = fs::remove_file(path);
}

#[test]
fn rejects_dependency_cycle() {
    let path = write_temp_toml(
        "cycle",
        r#"
[[units]]
name = "a"
exec = "/bin/a"
after = ["b"]

[[units]]
name = "b"
exec = "/bin/b"
after = ["a"]
"#,
    );

    let err = parse_units_file(&path).unwrap_err();
    assert!(err.contains("cycle"), "got: {err}");

    let _ = fs::remove_file(path);
}

#[test]
fn rejects_duplicate_name() {
    let path = write_temp_toml(
        "dup",
        r#"
[[units]]
name = "a"
exec = "/bin/a"

[[units]]
name = "a"
exec = "/bin/a2"
"#,
    );

    let err = parse_units_file(&path).unwrap_err();
    assert!(err.contains("duplicate"), "got: {err}");

    let _ = fs::remove_file(path);
}

#[test]
fn rejects_unknown_restart_value() {
    let path = write_temp_toml(
        "bad_restart",
        r#"
[[units]]
name = "a"
exec = "/bin/a"
restart = "sometimes"
"#,
    );

    let err = parse_units_file(&path).unwrap_err();
    assert!(err.contains("sometimes"), "got: {err}");

    let _ = fs::remove_file(path);
}

#[test]
fn rejects_missing_exec() {
    let path = write_temp_toml(
        "no_exec",
        r#"
[[units]]
name = "a"
"#,
    );

    assert!(parse_units_file(&path).is_err());

    let _ = fs::remove_file(path);
}

#[test]
fn rejects_missing_units_key() {
    let path = write_temp_toml("no_units", "not_units = []\n");

    assert!(parse_units_file(&path).is_err());

    let _ = fs::remove_file(path);
}

#[test]
fn session_history_limit_defaults_when_session_table_is_absent() {
    let path = write_temp_toml(
        "history_default",
        r#"
[[units]]
name = "a"
exec = "/bin/a"
"#,
    );
    let parsed = parse_units_file(&path).unwrap();
    assert_eq!(parsed.history_limit, DEFAULT_HISTORY_LIMIT);
    assert_eq!(parsed.history_limit, 10_000);
    let _ = fs::remove_file(path);
}

#[test]
fn session_history_limit_is_read() {
    for (index, value) in [(0, 100_u32), (1, 2500), (2, 1_000_000)] {
        let path = write_temp_toml(
            &format!("history_read_{index}"),
            &format!(
                "[session]\nhistoryLimit = {value}\n\n[[units]]\nname = \"a\"\nexec = \"/bin/a\"\n"
            ),
        );
        let parsed = parse_units_file(&path).unwrap();
        assert_eq!(parsed.history_limit, value);
        let _ = fs::remove_file(path);
    }
}

#[test]
fn rejects_history_limit_out_of_range() {
    for (index, value) in ["99", "1000001", "-5", "0"].iter().enumerate() {
        let path = write_temp_toml(
            &format!("history_range_{index}"),
            &format!(
                "[session]\nhistoryLimit = {value}\n\n[[units]]\nname = \"a\"\nexec = \"/bin/a\"\n"
            ),
        );
        let error = parse_units_file(&path).unwrap_err();
        assert!(error.contains("historyLimit"), "value {value}: {error}");
        let _ = fs::remove_file(path);
    }
}

#[test]
fn rejects_non_integer_history_limit() {
    for (index, value) in ["2.5", "\"big\""].iter().enumerate() {
        let path = write_temp_toml(
            &format!("history_type_{index}"),
            &format!(
                "[session]\nhistoryLimit = {value}\n\n[[units]]\nname = \"a\"\nexec = \"/bin/a\"\n"
            ),
        );
        assert!(parse_units_file(&path).is_err(), "value {value} was accepted");
        let _ = fs::remove_file(path);
    }
}

#[test]
fn rejects_unknown_session_key() {
    let path = write_temp_toml(
        "session_unknown_key",
        "[session]\nscrollback = 5\n\n[[units]]\nname = \"a\"\nexec = \"/bin/a\"\n",
    );
    let error = parse_units_file(&path).unwrap_err();
    assert!(error.contains("scrollback"), "{error}");
    let _ = fs::remove_file(path);
}

#[test]
fn launch_name_rules() {
    assert!(is_valid_launch_name("planner"));
    assert!(is_valid_launch_name("lidar_front-2"));
    assert!(!is_valid_launch_name(""));
    assert!(!is_valid_launch_name("a.b"));
    assert!(!is_valid_launch_name("a:b"));
    assert!(!is_valid_launch_name("a b"));
    assert!(!is_valid_launch_name("čidlo"));
}

#[test]
fn rejects_unit_name_that_tmux_cannot_target() {
    for (index, name) in ["a.b", "a:b", "a b", ""].iter().enumerate() {
        let path = write_temp_toml(
            &format!("unit_name_{index}"),
            &format!("[[units]]\nname = \"{name}\"\nexec = \"/bin/a\"\n"),
        );
        let error = parse_units_file(&path).unwrap_err();
        assert!(error.contains("unit name"), "name '{name}': {error}");
        let _ = fs::remove_file(path);
    }
}

#[test]
fn rejects_invalid_restart_delay() {
    for (index, value) in ["-1.0", "nan", "inf"].iter().enumerate() {
        let path = write_temp_toml(
            &format!("restart_delay_{index}"),
            &format!("[[units]]\nname = \"a\"\nexec = \"/bin/a\"\nrestartDelaySeconds = {value}\n"),
        );
        let error = parse_units_file(&path).unwrap_err();
        assert!(error.contains("restartDelaySeconds"), "value {value}: {error}");
        let _ = fs::remove_file(path);
    }
}

#[test]
fn accepts_zero_restart_delay() {
    let path = write_temp_toml(
        "restart_delay_zero",
        "[[units]]\nname = \"a\"\nexec = \"/bin/a\"\nrestartDelaySeconds = 0.0\n",
    );
    let parsed = parse_units_file(&path).unwrap();
    assert!(parsed.units[0].restart_delay_seconds.abs() < 1e-9);
    let _ = fs::remove_file(path);
}
