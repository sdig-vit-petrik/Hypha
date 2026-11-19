//! Every refusal of `hypha_node_create` names its cause in one error line. Each refusal
//! runs in a child process: the log engine is process-wide.
#![allow(clippy::expect_used)]

use std::process::Command;
use std::ptr;

use hypha_ffi::*;

const CHILD_VARIABLE: &str = "HYPHA_NODE_ERROR_CHILD";

fn child_stderr(test: &str) -> String {
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", test, "--nocapture", "--test-threads", "1"])
        .env(CHILD_VARIABLE, test)
        .output()
        .expect("run the child process");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "the child's assertions hold: {stderr}");
    stderr
}

fn in_child(test: &str) -> bool {
    std::env::var(CHILD_VARIABLE).is_ok_and(|value| value == test)
}

#[test]
fn a_node_name_that_is_not_text_is_refused_with_its_cause() {
    const TEST: &str = "a_node_name_that_is_not_text_is_refused_with_its_cause";
    if in_child(TEST) {
        let name = [0xff_u8, 0xfe];
        let mut node = ptr::null_mut();
        let status = unsafe {
            hypha_node_create(
                name.as_ptr().cast(),
                name.len(),
                ptr::null(),
                0,
                ptr::null(),
                &raw mut node,
            )
        };
        assert_eq!(status, HyphaStatus::InvalidArgument);
        return;
    }
    let stderr = child_stderr(TEST);
    let named = stderr.contains(
        "hypha: node creation refused: the node name or parameter file is not valid text",
    );
    assert!(named, "{stderr}");
}

#[test]
fn a_node_creation_refused_for_a_mismatched_configuration_logs_its_cause() {
    const TEST: &str = "a_node_creation_refused_for_a_mismatched_configuration_logs_its_cause";
    if in_child(TEST) {
        let dir = std::env::temp_dir().join(format!("node_error_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture directory");
        let loaded = dir.join("a.toml");
        std::fs::write(&loaded, "gain = 1\n").expect("fixture file");
        let loaded_text = loaded.to_str().expect("text path").to_owned();
        let mut config = ptr::null_mut();
        let created = unsafe {
            hypha_config_create(loaded_text.as_ptr().cast(), loaded_text.len(), &raw mut config)
        };
        assert_eq!(created, HyphaStatus::Ok);
        let other = "/elsewhere/b.toml";
        let name = "mismatch_node";
        let mut node = ptr::null_mut();
        let status = unsafe {
            hypha_node_create(
                name.as_ptr().cast(),
                name.len(),
                other.as_ptr().cast(),
                other.len(),
                config,
                &raw mut node,
            )
        };
        assert_eq!(status, HyphaStatus::InvalidArgument);
        unsafe { hypha_config_destroy(config) };
        std::fs::remove_dir_all(&dir).unwrap_or_default();
        return;
    }
    let stderr = child_stderr(TEST);
    let named = stderr
        .contains("hypha: node creation refused: the configuration was loaded from '")
        && stderr.contains("a.toml', not '/elsewhere/b.toml'");
    assert!(named, "{stderr}");
}
