//! `--base-dir`: the definitions a package already has (a package of the SDK that an
//! application adds to). They resolve the added definitions' field types and are never
//! emitted; a definition of a name the package has is refused.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/additive").join(name)
}

fn scratch_dir(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hypha_msggen_base_{case}_{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("remove stale scratch directory");
    }
    std::fs::create_dir_all(&dir).expect("create scratch directory");
    dir
}

fn utf8(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

/// Generates `base_msgs` additions: `msg`/`srv` are the directories of the added definitions.
fn generate(out: &Path, msg: Option<&str>, srv: Option<&str>) -> Output {
    let base = fixture_dir("base_msgs");
    let mut args =
        vec!["--package", "base_msgs", "--base-dir", utf8(&base), "--out-dir", utf8(out)];
    let (msg_dir, srv_dir);
    if let Some(name) = msg {
        msg_dir = fixture_dir(name);
        args.extend(["--msg-dir", utf8(&msg_dir)]);
    }
    if let Some(name) = srv {
        srv_dir = fixture_dir(name);
        args.extend(["--srv-dir", utf8(&srv_dir)]);
    }
    Command::new(env!("CARGO_BIN_EXE_hypha_msggen")).args(&args).output().expect("run hypha_msggen")
}

#[test]
fn an_added_definition_resolves_against_the_package_and_emits_nothing_of_it() {
    let out = scratch_dir("adds");
    let output = generate(&out, Some("added"), Some("added"));
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let extra = std::fs::read_to_string(out.join("base_msgs/Extra.h")).expect("the added message");
    assert!(extra.contains("base_msgs::Stamp stamp"), "the package's own type did not resolve");
    for header in ["Probe.h", "ProbeRequest.h", "ProbeResponse.h"] {
        assert!(out.join("base_msgs").join(header).is_file(), "{header} missing");
    }
    for header in ["Stamp.h", "Ping.h", "PingRequest.h", "PingResponse.h"] {
        assert!(
            !out.join("base_msgs").join(header).exists(),
            "{header} of the package was emitted again"
        );
    }
    std::fs::remove_dir_all(out).expect("remove scratch directory");
}

#[test]
fn a_message_of_a_name_the_package_has_is_refused() {
    let out = scratch_dir("clash_msg");
    let output = generate(&out, Some("clash_msg"), None);
    assert!(!output.status.success(), "a second base_msgs/Stamp was generated");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("base_msgs/Stamp is already defined"), "{error}");
    assert!(!out.join("base_msgs").exists(), "a header was written before the refusal");
    std::fs::remove_dir_all(out).expect("remove scratch directory");
}

#[test]
fn a_service_of_a_name_the_package_has_is_refused() {
    let out = scratch_dir("clash_srv");
    let output = generate(&out, None, Some("clash_srv"));
    assert!(!output.status.success(), "a second base_msgs/Ping was generated");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("base_msgs/Ping is already defined"), "{error}");
    std::fs::remove_dir_all(out).expect("remove scratch directory");
}

#[test]
fn a_message_named_like_a_half_of_a_service_of_the_package_is_refused() {
    let out = scratch_dir("clash_half");
    let output = generate(&out, Some("clash_half"), None);
    assert!(!output.status.success(), "base_msgs/PingResponse was generated twice");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("base_msgs/PingResponse is already defined"), "{error}");
    std::fs::remove_dir_all(out).expect("remove scratch directory");
}

#[test]
fn the_package_itself_as_a_dependency_is_still_refused() {
    let out = scratch_dir("self_dependency");
    let base = fixture_dir("base_msgs");
    let added = fixture_dir("added");
    let dependency = format!("base_msgs={}", utf8(&base));
    let output = Command::new(env!("CARGO_BIN_EXE_hypha_msggen"))
        .args(["--package", "base_msgs", "--srv-dir", utf8(&added), "--dependency", &dependency])
        .args(["--out-dir", utf8(&out)])
        .output()
        .expect("run hypha_msggen");
    assert!(!output.status.success());
    std::fs::remove_dir_all(out).expect("remove scratch directory");
}
