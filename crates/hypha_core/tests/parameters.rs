//! Immutable parameter configuration tests.

mod common;

use hypha_core::parameters::{ParamValue, Parameters};

const VALID: &str = r#"
enabled = true
speed_limit = 45.5
count = 10
node_name = "wheel_driver"

[drive]
mode = "safe"
"#;

#[test]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
fn scalar_parameters_are_flattened_and_typed() {
    let params = common::from_text(VALID, "test.toml").unwrap();
    assert_eq!(params.value("enabled"), Some(&ParamValue::Bool(true)));
    assert_eq!(params.value("speed_limit"), Some(&ParamValue::Float(45.5)));
    assert_eq!(params.value("count"), Some(&ParamValue::Int(10)));
    assert_eq!(params.value("node_name"), Some(&ParamValue::Text("wheel_driver".into())));
    assert_eq!(params.value("drive.mode"), Some(&ParamValue::Text("safe".into())));
}

#[test]
fn malformed_toml_and_non_scalars_are_rejected() {
    assert!(common::from_text("count = ", "syntax.toml").is_err());
    assert!(common::from_text("count=[1, {a = 1}]\n", "array.toml").is_err());
}

#[test]
#[allow(clippy::unwrap_used)]
fn environment_references_are_substituted_in_string_values() {
    let params =
        common::from_text("home = '${PATH}/hypha'\nunchanged = '$PATH'\n", "environment.toml")
            .unwrap();
    let path = std::env::var("PATH").unwrap();
    let expected = format!("{path}/hypha");
    assert_eq!(params.value("home"), Some(&ParamValue::Text(expected)));
    assert_eq!(params.value("unchanged"), Some(&ParamValue::Text("$PATH".into())));
}

#[test]
#[allow(clippy::unwrap_used)]
fn an_unset_variable_is_refused_naming_file_key_and_variable() {
    let source = "[ports.inputs.x]\ntopic = '${HYPHA_TEST_VARIABLE_THAT_MUST_NOT_EXIST}'\n";
    let missing = common::from_text(source, "missing.toml").unwrap_err();
    assert!(missing.path.ends_with("missing.toml"), "{}", missing.path);
    assert_eq!(missing.key, "ports.inputs.x.topic");
    assert_eq!(
        missing.reason,
        "environment variable `HYPHA_TEST_VARIABLE_THAT_MUST_NOT_EXIST` is unset"
    );
    let text = missing.to_string();
    assert!(text.starts_with("could not load TOML configuration `"), "{text}");
    assert!(text.ends_with("missing.toml` at `ports.inputs.x.topic`: environment variable `HYPHA_TEST_VARIABLE_THAT_MUST_NOT_EXIST` is unset"), "{text}");
}

#[test]
#[allow(clippy::unwrap_used)]
fn an_empty_variable_is_refused_naming_file_key_and_variable() {
    hypha_testkit::set_env("HYPHA_TEST_EMPTY_VARIABLE_K2", "");
    let source = "[[sensors]]\ntopic = '/imu${HYPHA_TEST_EMPTY_VARIABLE_K2}'\n";
    let empty = common::from_text(source, "empty.toml").unwrap_err();
    hypha_testkit::remove_env("HYPHA_TEST_EMPTY_VARIABLE_K2");
    assert!(empty.path.ends_with("empty.toml"), "{}", empty.path);
    assert_eq!(empty.key, "sensors[0].topic");
    assert_eq!(empty.reason, "environment variable `HYPHA_TEST_EMPTY_VARIABLE_K2` is empty");
}

#[test]
#[allow(clippy::unwrap_used)]
fn invalid_environment_references_are_rejected() {
    let unterminated = common::from_text("value = '${PATH'\n", "unterminated.toml").unwrap_err();
    assert_eq!(unterminated.reason, "unterminated environment reference");

    let invalid = common::from_text("value = '${BAD-NAME}'\n", "invalid.toml").unwrap_err();
    assert_eq!(invalid.reason, "invalid environment variable name");
}

#[test]
#[allow(clippy::expect_used, clippy::unwrap_used)]
fn configuration_can_be_loaded_from_a_file() {
    let path =
        std::env::temp_dir().join(format!("hypha_core_parameters_{}.toml", std::process::id()));
    std::fs::write(&path, VALID).expect("write fixture");
    let params = Parameters::from_toml_file(&path).expect("load fixture");
    std::fs::remove_file(&path).expect("remove fixture");
    assert_eq!(params.value("count"), Some(&ParamValue::Int(10)));
    assert!(Parameters::from_toml_file(&path).is_err());
}

#[test]
#[allow(clippy::unwrap_used)]
fn a_value_keeps_the_type_the_file_gave_it() {
    let params =
        common::from_text("quoted = \"5\"\nnumber = 5\nflag = true\n", "typed.toml").unwrap();

    assert_eq!(params.value("quoted"), Some(&ParamValue::Text("5".into())));
    assert_eq!(params.value("number"), Some(&ParamValue::Int(5)));
    assert_eq!(params.value("flag"), Some(&ParamValue::Bool(true)));
}

const WITH_LOG: &str = r#"
speed = 1

[log]
level = "info"
sinks = ["console", "file"]

[log.file]
path = "node.log"
max_size = 1048576
"#;

#[test]
#[allow(clippy::unwrap_used)]
fn the_log_table_stays_out_of_the_flat_parameters() {
    let params = common::from_text(WITH_LOG, "log.toml").unwrap();
    assert!(!params.has("log.level"));
    assert!(params.value("log.level").is_none());
    assert!(params.value("log.file.path").is_none());
    assert_eq!(params.names(), vec!["speed"]);
    assert_eq!(params.iter().count(), 1);
}

#[test]
#[allow(clippy::unwrap_used)]
fn the_log_table_is_returned_unchanged() {
    let params = common::from_text(WITH_LOG, "log.toml").unwrap();
    let parsed: toml::Table = toml::from_str(WITH_LOG).unwrap();
    assert_eq!(params.log_table(), parsed.get("log").and_then(toml::Value::as_table));
}

#[test]
#[allow(clippy::unwrap_used)]
fn a_file_without_a_log_table_gives_none() {
    let params = common::from_text(VALID, "plain.toml").unwrap();
    assert!(params.log_table().is_none());
}

#[test]
#[allow(clippy::unwrap_used)]
fn a_log_value_that_is_not_a_table_is_refused() {
    let error = common::from_text("log = \"debug\"\n", "bad.toml").unwrap_err();
    assert_eq!(error.key, "log");
    assert!(error.path.ends_with("bad.toml"), "{}", error.path);
    assert!(error.reason.contains("table"), "{}", error.reason);
}

#[test]
#[allow(clippy::unwrap_used)]
fn reserved_names_are_node_name_only() {
    let source = r#"
use_sim_time = true
rate_hz = 50
extra = "typo"

[node]
name = "planner"
unexpected = 1

[controller]
kp = 1.0
ki = 0.2
"#;
    let params = common::from_text(source, "unread.toml").unwrap();
    assert_eq!(params.value("rate_hz"), Some(&ParamValue::Int(50)));
    assert!(params.value("controller.kp").is_some());

    assert_eq!(
        params.unread_keys(),
        vec!["controller.ki", "extra", "node.unexpected", "use_sim_time"]
    );
    assert_eq!(params.unread_keys_under("controller"), vec!["controller.ki"]);
    assert_eq!(params.unread_keys_under("node"), vec!["node.unexpected"]);
}
