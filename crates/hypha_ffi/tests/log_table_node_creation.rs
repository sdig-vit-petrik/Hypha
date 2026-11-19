//! The `[log]` table at node creation. Each test needs its own process (nextest).
#![allow(clippy::expect_used)]

mod common;
use crate::common::unique;

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;

use hypha_ffi::*;

fn fixture_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(unique(label));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    dir
}

fn create(label: &str, file: &Path) -> (HyphaStatus, *mut HyphaNode) {
    let name = CString::new(unique(label)).expect("node name");
    let path = CString::new(file.to_string_lossy().as_bytes()).expect("parameter path");
    let mut node = ptr::null_mut();
    let status = unsafe {
        hypha_node_create(
            name.as_ptr(),
            name.as_bytes().len(),
            path.as_ptr(),
            path.as_bytes().len(),
            ptr::null(),
            &raw mut node,
        )
    };
    (status, node)
}

fn write_params(dir: &Path, file: &str, text: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, text).expect("write parameter fixture");
    path
}

fn sink_table(path: &str) -> String {
    format!("[log]\nconsole = false\n[log.sink.main]\npath = \"{path}\"\nlevel = \"info\"\n")
}

#[test]
fn invalid_log_table_is_rejected_before_transport_startup() {
    let dir = fixture_dir("invalid");
    let file = write_params(&dir, "p.toml", "[log]\nlevel = \"loud\"\n");
    let (status, node) = create("invalid", &file);
    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert!(node.is_null());
}

#[test]
fn file_without_log_table_keeps_the_configuration() {
    let dir = fixture_dir("absent");
    let with_log = write_params(&dir, "a.toml", &sink_table("kept.log"));
    let without_log = write_params(&dir, "b.toml", "gain = 1.0\n");
    let (status, node) = create("first", &with_log);
    assert_eq!(status, HyphaStatus::Ok);
    let (status_second, node_second) = create("second", &without_log);
    assert_eq!(status_second, HyphaStatus::Ok);
    log::warn!(target: "hypha", "kept marker");
    let text = std::fs::read_to_string(dir.join("kept.log")).expect("read sink file");
    assert!(text.contains("kept marker"), "sink file: {text}");
    unsafe { hypha_node_destroy(node_second) };
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn configuration_stays_after_node_destroy() {
    let dir = fixture_dir("stays");
    let file = write_params(&dir, "p.toml", &sink_table("out.log"));
    let (status, node) = create("stays", &file);
    assert_eq!(status, HyphaStatus::Ok);
    unsafe { hypha_node_destroy(node) };
    log::warn!(target: "hypha", "after destroy marker");
    let text = std::fs::read_to_string(dir.join("out.log")).expect("read sink file");
    assert!(text.contains("after destroy marker"), "sink file: {text}");
}

#[test]
fn relative_sink_path_resolves_against_the_parameter_file_directory() {
    let dir = fixture_dir("relative");
    let file = write_params(&dir, "p.toml", &sink_table("sub/rel.log"));
    let (status, node) = create("relative", &file);
    assert_eq!(status, HyphaStatus::Ok);
    assert!(dir.join("sub/rel.log").is_file());
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn different_log_table_of_a_second_node_is_refused() {
    let dir = fixture_dir("different");
    let first = write_params(&dir, "a.toml", &sink_table("a.log"));
    let second = write_params(&dir, "b.toml", &sink_table("b.log"));
    let (status, node) = create("first", &first);
    assert_eq!(status, HyphaStatus::Ok);
    let (status_second, node_second) = create("second", &second);
    assert_eq!(status_second, HyphaStatus::InvalidState);
    assert!(node_second.is_null());
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn equal_log_table_of_a_second_node_is_accepted() {
    let dir = fixture_dir("equal");
    let first = write_params(&dir, "a.toml", &sink_table("a.log"));
    let (status, node) = create("first", &first);
    assert_eq!(status, HyphaStatus::Ok);
    let (status_second, node_second) = create("second", &first);
    assert_eq!(status_second, HyphaStatus::Ok);
    unsafe { hypha_node_destroy(node_second) };
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn failed_node_creation_leaves_the_configuration_unchanged() {
    let dir = fixture_dir("failed");
    let file = write_params(&dir, "p.toml", &format!("node.name = 5\n{}", sink_table("never.log")));
    let (status, node) = create("failed", &file);
    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert!(node.is_null());
    // The configuration did not change: a different table is still accepted.
    let other = write_params(&dir, "o.toml", &sink_table("other.log"));
    let (status_other, node_other) = create("other", &other);
    assert_eq!(status_other, HyphaStatus::Ok);
    unsafe { hypha_node_destroy(node_other) };
}

/// The line a node on simulated time writes once per process.
const SIM_TIME_WARNING: &str =
    "simulated time is on (HYPHA_USE_SIM_TIME): this node follows /clock";

/// Create two nodes from one file with a sink, and return the sink file's lines.
fn sink_lines_of_two_nodes(label: &str, sim_time: bool) -> Vec<String> {
    if sim_time {
        hypha_testkit::set_env("HYPHA_USE_SIM_TIME", "1");
    } else {
        hypha_testkit::remove_env("HYPHA_USE_SIM_TIME");
    }
    let dir = fixture_dir(label);
    let file = write_params(&dir, "p.toml", &sink_table("out.log"));
    let (status, node) = create(label, &file);
    assert_eq!(status, HyphaStatus::Ok);
    let (status_second, node_second) = create(label, &file);
    assert_eq!(status_second, HyphaStatus::Ok);
    unsafe { hypha_node_destroy(node_second) };
    unsafe { hypha_node_destroy(node) };
    hypha_testkit::remove_env("HYPHA_USE_SIM_TIME");
    let text = std::fs::read_to_string(dir.join("out.log")).unwrap_or_default();
    text.lines().map(str::to_owned).collect()
}

#[test]
fn a_sim_time_node_writes_the_warning_once_after_the_log_table() {
    let lines = sink_lines_of_two_nodes("sim_warning", true);
    let warnings: Vec<&String> =
        lines.iter().filter(|line| line.contains(SIM_TIME_WARNING)).collect();
    assert_eq!(warnings.len(), 1, "sink lines: {lines:?}");
    let expected_tail = format!("[WARN] [hypha] {SIM_TIME_WARNING}");
    let exact = warnings.first().is_some_and(|line| line.ends_with(&expected_tail));
    assert!(exact, "lines: {warnings:?}");
}

#[test]
fn a_host_time_node_writes_no_such_line() {
    let lines = sink_lines_of_two_nodes("host_no_warning", false);
    let warned = lines.iter().any(|line| line.contains("simulated time is on"));
    assert!(!warned, "sink lines: {lines:?}");
}

#[test]
fn a_log_path_expands_its_environment_references() {
    let dir = fixture_dir("log_env");
    hypha_testkit::set_env("HYPHA_TEST_LOG_DIR_K2", dir.to_str().expect("fixture path is text"));
    let file = write_params(&dir, "p.toml", &sink_table("${HYPHA_TEST_LOG_DIR_K2}/n.log"));
    let (status, node) = create("log_env", &file);
    hypha_testkit::remove_env("HYPHA_TEST_LOG_DIR_K2");
    assert_eq!(status, HyphaStatus::Ok);
    log::warn!(target: "hypha", "expanded marker");
    let text = std::fs::read_to_string(dir.join("n.log")).unwrap_or_default();
    assert!(text.contains("expanded marker"), "sink file: {text}");
    let literal_dir_exists = dir.join("${HYPHA_TEST_LOG_DIR_K2}").exists();
    assert!(!literal_dir_exists, "a directory named after the reference was created");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_log_path_with_an_empty_variable_stops_node_creation() {
    let dir = fixture_dir("log_env_empty");
    hypha_testkit::set_env("HYPHA_TEST_LOG_EMPTY_K2", "");
    let file = write_params(&dir, "p.toml", &sink_table("${HYPHA_TEST_LOG_EMPTY_K2}/n.log"));
    let (status, node) = create("log_env_empty", &file);
    hypha_testkit::remove_env("HYPHA_TEST_LOG_EMPTY_K2");
    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert!(node.is_null());
    let opened = std::fs::read_dir(&dir).map_or(0, Iterator::count);
    assert_eq!(opened, 1, "only the parameter file exists in {}", dir.display());
}

#[test]
fn a_log_sink_path_in_an_included_file_resolves_against_that_file() {
    let dir = fixture_dir("sink_in_include");
    std::fs::create_dir_all(dir.join("logs")).expect("create the include directory");
    write_params(&dir, "logs/a.toml", &sink_table("out.log"));
    let root = write_params(&dir, "p.toml", "include = [\"logs/a.toml\"]\n");
    let (status, node) = create("sink_in_include", &root);
    assert_eq!(status, HyphaStatus::Ok);
    log::warn!(target: "hypha", "include marker");
    let text = std::fs::read_to_string(dir.join("logs/out.log")).unwrap_or_default();
    assert!(text.contains("include marker"), "sink file: {text}");
    let beside_root = dir.join("out.log").exists();
    assert!(!beside_root, "the sink opened beside the root file");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn an_override_writes_one_debug_line() {
    let dir = fixture_dir("override_line");
    write_params(&dir, "base.toml", "rate_hz = 50\n");
    let table = "[log]\nconsole = false\n[log.sink.main]\npath = \"out.log\"\nlevel = \"debug\"\n";
    let text = format!("include = [\"base.toml\"]\nrate_hz = 20\n{table}");
    let root = write_params(&dir, "p.toml", &text);
    let (status, node) = create("override_line", &root);
    assert_eq!(status, HyphaStatus::Ok);
    unsafe { hypha_node_destroy(node) };
    let sink = std::fs::read_to_string(dir.join("out.log")).unwrap_or_default();
    let expected = format!(
        "[DEBUG] [hypha] hypha: rate_hz from {} overrides {}",
        root.display(),
        dir.join("base.toml").display()
    );
    let matching = sink.lines().filter(|line| line.ends_with(&expected)).count();
    assert_eq!(matching, 1, "sink file: {sink}");
}
