//! A parameter file that `hypha_config_create` refuses is reported on standard error.
//! The refusal runs in a child process: the log engine is process-wide.

#![allow(clippy::expect_used)]

use std::process::Command;

use hypha_ffi::*;

const CHILD_VARIABLE: &str = "HYPHA_CONFIG_ERROR_CHILD";
const TEST_NAME: &str = "a_refused_configuration_is_reported_on_standard_error";
const UNSET_VARIABLE: &str = "HYPHA_TEST_UNSET_K2_CONFIG";

/// The body of the child: load a file that references an unset variable.
fn child() {
    let path = std::env::temp_dir().join(format!("config_error_{}.toml", std::process::id()));
    std::fs::write(&path, format!("[ports.inputs.x]\ntopic = '${{{UNSET_VARIABLE}}}'\n"))
        .expect("write the parameter file");
    let text = path.to_str().expect("temp path is text").to_owned();
    let mut config = std::ptr::null_mut();
    let status = unsafe { hypha_config_create(text.as_ptr().cast(), text.len(), &raw mut config) };
    std::fs::remove_file(&path).unwrap_or_default();
    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert!(config.is_null());
}

#[test]
fn a_refused_configuration_is_reported_on_standard_error() {
    if std::env::var_os(CHILD_VARIABLE).is_some() {
        child();
        return;
    }
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", TEST_NAME, "--nocapture", "--test-threads", "1"])
        .env(CHILD_VARIABLE, "1")
        .env_remove(UNSET_VARIABLE)
        .output()
        .expect("run the child process");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the child's assertions hold: {stderr}");
    let expected = format!(
        "toml` at `ports.inputs.x.topic`: environment variable `{UNSET_VARIABLE}` is unset"
    );
    let named =
        stderr.contains("could not load TOML configuration `") && stderr.contains(&expected);
    assert!(named, "the refusal names file, key and variable: {stderr}");
}
