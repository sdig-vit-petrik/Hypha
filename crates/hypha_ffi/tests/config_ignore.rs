//! `hypha_config_ignore` and the file's `unused` list: ignored keys leave the unread
//! keys and show in the node manifest with their reason (#119).
#![allow(clippy::expect_used, reason = "test setup must succeed")]

mod common;
use crate::common::introspection::snapshot;
use crate::common::{c_text, unique};

use std::path::PathBuf;
use std::ptr;

use hypha_ffi::*;
use hypha_msgs::hypha_msgs::IntrospectedConfig;

/// A parameter file under the temp directory, removed when the test ends.
struct ParameterFile(PathBuf);

impl ParameterFile {
    fn new(label: &str, body: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("hypha_config_ignore_{}_{label}.toml", std::process::id()));
        std::fs::write(&path, body).expect("write the parameter file");
        Self(path)
    }

    fn text(&self) -> &str {
        self.0.to_str().expect("a UTF-8 path")
    }
}

impl Drop for ParameterFile {
    fn drop(&mut self) {
        // Cleanup must not panic while a failing test is already unwinding.
        let _ = std::fs::remove_file(&self.0);
    }
}

fn create_config(file: &ParameterFile) -> *mut HyphaConfig {
    let path = file.text();
    let mut config = ptr::null_mut();
    // SAFETY: `path` holds `path.len()` bytes and `config` is one writable pointer.
    let status = unsafe { hypha_config_create(path.as_ptr().cast(), path.len(), &raw mut config) };
    assert_eq!(status, HyphaStatus::Ok);
    config
}

/// Creates the node `name` on `file`, with the prepared `config` or none.
fn create_node(
    name: &str,
    file: &ParameterFile,
    config: *const HyphaConfig,
) -> (HyphaStatus, *mut HyphaNode) {
    let name_text = c_text(name);
    let path = file.text();
    let mut node = ptr::null_mut();
    // SAFETY: both texts hold the lengths passed, `config` is null or live, and `node`
    // is one writable pointer.
    let status = unsafe {
        hypha_node_create(
            name_text.as_ptr(),
            name_text.as_bytes().len(),
            path.as_ptr().cast(),
            path.len(),
            config,
            &raw mut node,
        )
    };
    (status, node)
}

fn unread(config: *const HyphaConfig) -> String {
    let mut buffer = vec![0_u8; 4096];
    let mut length = 0_usize;
    // SAFETY: `config` is live, `buffer` has `buffer.len()` writable bytes and `length`
    // is one writable `usize`.
    let status = unsafe {
        hypha_config_unread_keys(
            config,
            ptr::null(),
            0,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut length,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);
    buffer.truncate(length);
    String::from_utf8(buffer).expect("UTF-8 keys")
}

fn ignore(config: *const HyphaConfig, key: &str, reason: &str) -> HyphaStatus {
    // SAFETY: `config` is live and both texts hold the lengths passed.
    unsafe {
        hypha_config_ignore(
            config,
            key.as_ptr().cast(),
            key.len(),
            reason.as_ptr().cast(),
            reason.len(),
        )
    }
}

fn ignored_rows(node: *const HyphaNode) -> Vec<(String, String)> {
    snapshot(node)
        .configuration
        .into_iter()
        .filter(|row| row.source == IntrospectedConfig::SOURCE_IGNORED)
        .map(|row| (row.key, row.value))
        .collect()
}

fn destroy_config(config: *mut HyphaConfig) {
    // SAFETY: `config` came from `hypha_config_create` and is not used afterwards.
    unsafe { hypha_config_destroy(config) };
}

#[test]
fn an_ignored_subtree_leaves_the_unread_keys_and_a_typo_stays() {
    let file = ParameterFile::new("subtree", "typo = 1\n[model_params]\nprecision = 'fp32'\n");
    let config = create_config(&file);
    assert_eq!(unread(config), "model_params.precision\ntypo");
    assert_eq!(ignore(config, "model_params", "no TensorRT in this build"), HyphaStatus::Ok);
    assert_eq!(ignore(config, "absent", "a branch not taken"), HyphaStatus::Ok);
    assert_eq!(unread(config), "typo");
    destroy_config(config);
}

#[test]
fn an_empty_key_or_reason_is_refused_and_ignores_nothing() {
    let file = ParameterFile::new("empty", "typo = 1\n");
    let config = create_config(&file);
    assert_eq!(ignore(config, "", "everything"), HyphaStatus::InvalidArgument);
    assert_eq!(ignore(config, "typo", ""), HyphaStatus::InvalidArgument);
    assert_eq!(unread(config), "typo");
    destroy_config(config);
}

#[test]
fn file_and_code_ignores_are_ignored_rows_of_the_node_manifest() {
    let file = ParameterFile::new(
        "manifest",
        "unused = ['vendor.stale']\n[vendor]\nstale = 1\n[model_params]\nprecision = 'fp32'\n\
         [diagnostic_updater]\nperiod = 0.5\n",
    );
    // Before the node: as runNode() reads its configuration while the application is built.
    let config = create_config(&file);
    assert_eq!(ignore(config, "model_params", "no TensorRT in this build"), HyphaStatus::Ok);
    let (status, node) = create_node(&unique("config_ignore"), &file, config);
    assert_eq!(status, HyphaStatus::Ok);
    // After the node: through the node's own configuration.
    // SAFETY: `node` is live.
    let node_config = unsafe { hypha_node_config(node) };
    assert_eq!(
        ignore(node_config, "diagnostic_updater.period", "the period is the one code gives"),
        HyphaStatus::Ok
    );
    assert_eq!(unread(node_config), "");
    let rows = ignored_rows(node);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows.iter().any(|(key, reason)| key == "vendor.stale" && reason.contains("unused")));
    assert!(rows.contains(&("model_params".to_owned(), "no TensorRT in this build".to_owned())));
    assert!(rows.contains(&(
        "diagnostic_updater.period".to_owned(),
        "the period is the one code gives".to_owned()
    )));
    // SAFETY: `node` is live and not used afterwards.
    unsafe { hypha_node_destroy(node) };
    destroy_config(config);
}

#[test]
fn a_listed_key_the_file_does_not_set_refuses_the_node() {
    let file =
        ParameterFile::new("stale", "unused = ['pose_measure_uncertainty_time']\nrate = 1\n");
    let (status, node) = create_node(&unique("config_ignore_stale"), &file, ptr::null());
    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert!(node.is_null());
}
