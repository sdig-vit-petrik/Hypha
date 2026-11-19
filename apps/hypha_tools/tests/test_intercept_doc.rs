//! The intercept config file and the state document.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::PathBuf;

use hypha_tools::intercept_doc::{
    BackendState, InterceptConfig, InterceptionSpec, InterceptionState, OverrideSpec, RelayState,
    load_config, parse_config, state_topic, write_config_atomic,
};
use hypha_tools::msg_patch::Override;

const CONTRACT: &str = r#"
revision = 7                       # bumped by every writer; the backend echoes it once applied

[[interception]]
name   = "battery"                 # unique, [a-z0-9_]+
input  = "/battery/raw"
output = "/battery"
armed  = true                      # false = bypass: forward unchanged

[interception.overrides]
"cells[2].voltage" = { value = "0.0" }
"status"           = { set = "0x0002", clear = "0x0008" }
"serial_number"    = { value = "ZZZ\\x00" }
"#;

fn temp_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("intercept_doc_{test}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn spec(name: &str, input: &str, output: &str) -> InterceptionSpec {
    InterceptionSpec {
        name: name.to_owned(),
        input: input.to_owned(),
        output: output.to_owned(),
        armed: true,
        overrides: std::collections::BTreeMap::new(),
    }
}

#[test]
fn the_contract_example_parses() {
    let config = parse_config(CONTRACT).expect("contract parses");
    assert_eq!(config.revision, 7);
    assert_eq!(config.interception.len(), 1);
    let battery = &config.interception[0];
    assert_eq!(battery.overrides.len(), 3);
    assert_eq!(battery.overrides["status"].to_override(), Ok(Override::Bits { set: 2, clear: 8 }));
    assert_eq!(
        battery.overrides["serial_number"].to_override(),
        Ok(Override::Value("ZZZ\\x00".to_owned()))
    );
    let only_set = parse_config(
        "[[interception]]\nname='a'\ninput='/a'\noutput='/b'\n[interception.overrides]\n's' = { set = '1' }\n",
    )
    .expect("clear defaults to 0");
    assert_eq!(
        only_set.interception[0].overrides["s"].to_override(),
        Ok(Override::Bits { set: 1, clear: 0 })
    );
    assert!(only_set.interception[0].armed, "armed defaults to true");
}

#[test]
fn config_round_trips_through_toml() {
    let config = parse_config(CONTRACT).expect("contract parses");
    let text = toml::to_string(&config).expect("serialize");
    assert_eq!(parse_config(&text), Ok(config));
}

#[test]
fn state_round_trips_through_toml() {
    let state = BackendState {
        backend: "bench".to_owned(),
        config_path: "/abs/intercept.toml".to_owned(),
        applied_revision: 7,
        config_error: String::new(),
        interception: vec![InterceptionState {
            name: "battery".to_owned(),
            state: RelayState::SchemaChanged,
            data_type: "sensor_msgs/BatteryState".to_owned(),
            received: 3,
            modified: 2,
            published: 3,
            patch_failed: 1,
            unresolved_dropped: 0,
            last_error: "status: x".to_owned(),
        }],
    };
    let text = toml::to_string(&state).expect("serialize");
    assert!(text.contains("state = \"schema_changed\""), "{text}");
    assert_eq!(toml::from_str::<BackendState>(&text).expect("parse"), state);
    assert_eq!(state_topic("bench"), "/hypha/intercept/bench/state");
}

#[test]
fn bad_names_duplicates_and_loops_are_refused() {
    let cases = [
        vec![spec("Battery", "/a", "/b")],
        vec![spec("x", "/a", "/b"), spec("x", "/c", "/d")],
        vec![spec("loop", "/a", "/a")],
        vec![spec("one", "/a", "/out"), spec("two", "/b", "/out")],
    ];
    for interception in cases {
        let name = interception.last().unwrap().name.clone();
        let text = toml::to_string(&InterceptConfig { revision: 1, interception }).unwrap();
        let error = parse_config(&text).expect_err(&name);
        assert!(error.contains(&name), "{error}");
    }
    assert!(parse_config("revision = [").is_err());
}

#[test]
fn missing_file_is_an_empty_config() {
    let dir = temp_dir("missing");
    assert_eq!(load_config(&dir.join("intercept.toml")), Ok(InterceptConfig::default()));
}

#[test]
fn atomic_write_replaces_and_leaves_no_temp() {
    let dir = temp_dir("atomic");
    let path = dir.join("intercept.toml");
    let mut config = parse_config(CONTRACT).expect("contract parses");
    write_config_atomic(&path, &config).expect("first write");
    config.revision = 8;
    config.interception[0]
        .overrides
        .insert("status".to_owned(), OverrideSpec::Value { value: "1".to_owned() });
    write_config_atomic(&path, &config).expect("second write");
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["intercept.toml"]);
    assert_eq!(load_config(&path), Ok(config));
}
