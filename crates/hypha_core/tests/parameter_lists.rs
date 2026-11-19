//! Native TOML arrays: a list of tables or of scalars, read by name and index.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

mod common;

use hypha_core::parameters::{ListElement, ParamValue, ParameterLoadError, Parameters};

const TOPIC_VARIABLE: &str = "HYPHA_TEST_LIST_TOPIC";
const NESTED_REASON: &str = "lists inside list elements are not supported";
const MIXED_REASON: &str = "a list cannot mix scalars and tables";
const CONFLICT_REASON: &str = "conflicts with scalar";

/// A directory under the system temp directory. It is removed when the test ends.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("hypha_lists_{}_{label}", std::process::id()));
        // A fixture left by an earlier run may be absent; that is the normal case.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture directory");
        Self { dir }
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.dir.join(relative);
        std::fs::write(&path, text).expect("write fixture file");
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Cleanup must not panic while a failing test is already unwinding.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn load(source: &str) -> Parameters {
    common::from_text(source, "lists.toml").expect("load lists")
}

fn refused(source: &str) -> ParameterLoadError {
    common::from_text(source, "lists.toml").expect_err("the list must be refused")
}

fn table_element<'a>(params: &'a Parameters, name: &str, index: usize) -> &'a Parameters {
    params.list_element(name, index).and_then(ListElement::table).expect("a table element")
}

fn text(value: &str) -> ParamValue {
    ParamValue::Text(value.to_string())
}

#[test]
fn an_array_of_tables_keeps_toml_order_and_exposes_each_table() {
    let params = load(
        "[[sensors]]\ntopic = \"/imu\"\nrate = 100\n\n[[sensors]]\ntopic = \"/gps\"\nrate = 10\n",
    );
    assert_eq!(params.list_len("sensors"), Some(2));
    assert!(params.list_element("sensors", 2).is_none());
    let first = table_element(&params, "sensors", 0);
    assert_eq!(first.value("topic"), Some(&ParamValue::Text("/imu".into())));
    assert_eq!(first.value("rate"), Some(&ParamValue::Int(100)));
    assert_eq!(first.names(), ["rate", "topic"]);
    assert_eq!(
        table_element(&params, "sensors", 1).value("topic"),
        Some(&ParamValue::Text("/gps".into()))
    );
}

#[test]
fn an_array_of_scalars_gives_scalar_elements() {
    let params = load(
        "sensors = [\"imu\", \"gps\"]\ncounts = [1, 2]\nrates = [0.5, 1.5]\nflags = [true, false]\n",
    );
    let scalar = |name: &str, index: usize| {
        params.list_element(name, index).and_then(ListElement::scalar).cloned()
    };
    assert_eq!(scalar("sensors", 1), Some(text("gps")));
    assert_eq!(scalar("counts", 0), Some(ParamValue::Int(1)));
    assert_eq!(scalar("rates", 1), Some(ParamValue::Float(1.5)));
    assert_eq!(scalar("flags", 1), Some(ParamValue::Bool(false)));
    assert!(params.list_element("counts", 0).and_then(ListElement::table).is_none());
}

#[test]
fn an_empty_array_is_a_list_of_length_zero() {
    let params = load("sensors = []\n");
    assert_eq!(params.list_len("sensors"), Some(0));
    assert!(params.list_element("sensors", 0).is_none());
    assert_eq!(params.list_names(), ["sensors"]);
}

#[test]
fn a_list_under_a_table_takes_the_dotted_name() {
    let params = load("[ports]\nsensors = [\"imu\"]\n");
    assert_eq!(params.list_len("ports.sensors"), Some(1));
    assert_eq!(params.list_len("sensors"), None);
    assert_eq!(params.list_names(), ["ports.sensors"]);
}

#[test]
fn environment_references_fill_scalar_and_table_list_elements() {
    hypha_testkit::set_env(TOPIC_VARIABLE, "/from_env");
    let params = load(
        "names = [\"${HYPHA_TEST_LIST_TOPIC}\"]\n[[sensors]]\ntopic = \"${HYPHA_TEST_LIST_TOPIC}/imu\"\n",
    );
    assert_eq!(
        params.list_element("names", 0).and_then(ListElement::scalar),
        Some(&text("/from_env"))
    );
    let topic = table_element(&params, "sensors", 0).value("topic");
    assert_eq!(topic, Some(&text("/from_env/imu")));
}

#[test]
fn a_missing_environment_variable_names_the_element_field() {
    let err = refused("[[sensors]]\ntopic = \"${HYPHA_TEST_LIST_UNSET}\"\n");
    assert_eq!(err.key, "sensors[0].topic");
}

#[test]
fn a_duplicate_list_across_included_files_is_refused() {
    let fixture = Fixture::new("replace");
    fixture.write("a.toml", "[limits]\nmax = 1\n[[cells]]\nid = 1\n[[cells]]\nid = 2\n");
    fixture.write("b.toml", "[limits]\nmin = 0\n[[cells]]\nid = 3\n");
    let root = fixture.write("root.toml", "include = [\"a.toml\", \"b.toml\"]\n");
    let err = Parameters::from_toml_file(&root).expect_err("duplicate list must be refused");
    assert_eq!(err.key, "cells");
    assert!(err.reason.contains("conflict"), "{}", err.reason);
}

#[test]
fn a_root_list_replaces_an_include_list() {
    let fixture = Fixture::new("root_list");
    fixture.write("a.toml", "names = [\"x\", \"y\"]\n[[cells]]\nid = 1\n[[cells]]\nid = 2\n");
    let root =
        fixture.write("root.toml", "include = [\"a.toml\"]\nnames = [\"z\"]\n[[cells]]\nid = 3\n");
    let params = Parameters::from_toml_file(&root).expect("the root list replaces the include's");
    assert_eq!(params.list_len("cells"), Some(1));
    assert_eq!(params.list_len("names"), Some(1));
}

#[test]
fn an_included_file_with_a_mixed_list_is_refused() {
    let fixture = Fixture::new("malformed");
    fixture.write("bad.toml", "sensors = [1, { topic = \"x\" }]\n");
    let root = fixture.write("root.toml", "include = [\"bad.toml\"]\n");
    let err = Parameters::from_toml_file(&root).expect_err("a mixed list is refused");
    assert_eq!(err.key, "sensors");
    assert_eq!(err.reason, MIXED_REASON);
}

#[test]
fn a_list_that_mixes_scalars_and_tables_is_refused_either_way() {
    for source in ["sensors = [1, { topic = \"x\" }]\n", "sensors = [{ topic = \"x\" }, 1]\n"] {
        let err = refused(source);
        assert_eq!(err.key, "sensors");
        assert_eq!(err.reason, MIXED_REASON);
    }
}

#[test]
fn an_array_inside_a_scalar_element_is_refused() {
    let err = refused("sensors = [[1, 2]]\n");
    assert_eq!(err.key, "sensors[0]");
    assert_eq!(err.reason, NESTED_REASON);
}

#[test]
fn an_array_inside_a_table_element_is_refused() {
    let err = refused("[[sensors]]\ninner = [1]\n");
    assert_eq!(err.key, "sensors[0].inner");
    assert_eq!(err.reason, NESTED_REASON);
}

#[test]
fn a_scalar_key_and_a_list_of_the_same_name_are_refused() {
    assert!(common::from_text("sensors = 1\n[[sensors]]\ntopic = \"x\"\n", "dup.toml").is_err());
}

#[test]
fn a_dotted_scalar_key_below_a_list_is_refused() {
    let err = refused("\"sensors.topic\" = \"x\"\n[[sensors]]\ntopic = \"y\"\n");
    assert_eq!(err.key, "sensors");
    assert!(err.reason.contains(CONFLICT_REASON), "{}", err.reason);
}

#[test]
fn a_list_below_a_scalar_key_is_refused() {
    let err = refused("ports = 1\n\"ports.sensors\" = [\"imu\"]\n");
    assert_eq!(err.key, "ports.sensors");
    assert!(err.reason.contains(CONFLICT_REASON), "{}", err.reason);
}

#[test]
fn a_list_and_a_scalar_of_the_same_dotted_name_are_refused() {
    let err = refused("\"ports.sensors\" = 1\n[ports]\nsensors = [\"imu\"]\n");
    assert_eq!(err.key, "ports.sensors");
    assert!(err.reason.contains(CONFLICT_REASON), "{}", err.reason);
}

#[test]
fn list_names_are_sorted_and_scalar_queries_do_not_see_lists() {
    let params =
        load("sensors = [\"imu\"]\nb = 1\n[ports]\nsensors = [\"gps\"]\n[[cells]]\nid = 1\n");
    assert_eq!(params.list_names(), ["cells", "ports.sensors", "sensors"]);
    assert_eq!(params.names(), ["b"]);
    assert!(!params.has("sensors"));
    assert!(params.value("sensors").is_none());
    let scalar_names: Vec<&str> = params.iter().map(|(name, _)| name).collect();
    assert_eq!(scalar_names, ["b"]);
}

#[test]
fn a_clone_keeps_its_lists() {
    let params = load("[[sensors]]\ntopic = \"/imu\"\n");
    assert_eq!(params.clone(), params);
}

#[test]
fn unread_list_and_table_element_keys_are_reported() {
    let source = r#"
unused_scalars = [1, 2]
read_scalars = [10, 20]
empty_list = []

[[sensors]]
id = "imu0"
topic = "/imu"
typo = "bad"

[[sensors]]
id = "gps0"
topic = "/gps"
"#;
    let params = load(source);
    assert_eq!(params.list_len("empty_list"), Some(0));
    assert!(params.list_element("read_scalars", 0).is_some());
    let first = params.list_element("sensors", 0).unwrap().table().unwrap();
    assert!(first.value("id").is_some());
    assert!(first.value("topic").is_some());
    let second = params.list_element("sensors", 1).unwrap().table().unwrap();
    assert!(second.value("id").is_some());

    assert_eq!(
        params.unread_keys(),
        vec!["read_scalars[1]", "sensors[0].typo", "sensors[1].topic", "unused_scalars"]
    );
    assert_eq!(params.unread_keys_under("sensors[0]"), vec!["sensors[0].typo"]);
    assert_eq!(params.unread_keys_under("sensors"), vec!["sensors[0].typo", "sensors[1].topic"]);
}
