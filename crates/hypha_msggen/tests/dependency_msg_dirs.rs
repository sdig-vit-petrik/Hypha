//! `--dependency`: message packages the generator reads to resolve a field type, and
//! does not emit code for, so one include namespace keeps one owner.

use std::path::PathBuf;
use std::process::Command;

fn fixture_dir(package: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dependency").join(package)
}

fn output_dir(case: &str) -> PathBuf {
    std::env::temp_dir().join(format!("hypha_msggen_dependency_{case}_{}", std::process::id()))
}

#[allow(clippy::expect_used)]
fn run_generator(args: &[&str]) -> bool {
    Command::new(env!("CARGO_BIN_EXE_hypha_msggen"))
        .args(args)
        .status()
        .expect("run hypha_msggen")
        .success()
}

#[test]
#[allow(clippy::expect_used)]
fn dependency_resolves_a_foreign_field_without_emitting_its_package() {
    let output = output_dir("resolves");
    if output.exists() {
        std::fs::remove_dir_all(&output).expect("remove stale generator output");
    }
    let app = fixture_dir("app_msgs");
    let base = fixture_dir("base_msgs");

    let dependency = format!("base_msgs={}", base.to_str().expect("UTF-8 fixture"));
    let generated = run_generator(&[
        "--package",
        "app_msgs",
        "--msg-dir",
        app.to_str().expect("UTF-8 fixture"),
        "--dependency",
        &dependency,
        "--out-dir",
        output.to_str().expect("UTF-8 output"),
    ]);
    assert!(generated, "generator refused a message whose field type came from a dependency");

    let header = output.join("app_msgs/Reading.h");
    assert!(header.is_file(), "the owned package's header was not written");

    let emitted = std::fs::read_to_string(&header).expect("read generated header");
    assert!(
        emitted.contains("base_msgs::Stamp stamp"),
        "the dependency's type did not resolve into the owned message"
    );

    assert!(
        !output.join("base_msgs").exists(),
        "the dependency package was emitted; it is owned and installed elsewhere"
    );

    std::fs::remove_dir_all(output).expect("remove generator output");
}

/// Without the dependency the generator refuses rather than emit a header naming a type
/// it never read.
#[test]
#[allow(clippy::expect_used)]
fn a_missing_dependency_refuses_the_generation() {
    let output = output_dir("refuses");
    if output.exists() {
        std::fs::remove_dir_all(&output).expect("remove stale generator output");
    }
    let app = fixture_dir("app_msgs");

    let generated = run_generator(&[
        "--package",
        "app_msgs",
        "--msg-dir",
        app.to_str().expect("UTF-8 fixture"),
        "--out-dir",
        output.to_str().expect("UTF-8 output"),
    ]);
    assert!(!generated, "generator accepted a message whose field type it could not resolve");

    if output.exists() {
        std::fs::remove_dir_all(output).expect("remove generator output");
    }
}

#[test]
#[allow(clippy::expect_used)]
fn a_package_name_that_leaves_the_output_directory_is_refused() {
    let output = output_dir("escape").join("inside");
    let app = fixture_dir("app_msgs");
    let base = fixture_dir("base_msgs");
    let dependency = format!("base_msgs={}", base.to_str().expect("UTF-8 fixture"));

    let generated = run_generator(&[
        "--package",
        "../escaped",
        "--msg-dir",
        app.to_str().expect("UTF-8 fixture"),
        "--dependency",
        &dependency,
        "--out-dir",
        output.to_str().expect("UTF-8 output"),
    ]);

    assert!(!generated, "a package name with a path was accepted");
    let escaped = output.join("../escaped");
    assert!(!escaped.exists(), "files were written outside the output directory");
}

#[test]
#[allow(clippy::expect_used)]
fn one_package_named_twice_as_a_dependency_is_refused() {
    let output = output_dir("twice");
    let app = fixture_dir("app_msgs");
    let base = fixture_dir("base_msgs");
    let first = format!("base_msgs={}", base.to_str().expect("UTF-8 fixture"));
    let second = format!("base_msgs={}", app.to_str().expect("UTF-8 fixture"));

    let generated = run_generator(&[
        "--package",
        "app_msgs",
        "--msg-dir",
        app.to_str().expect("UTF-8 fixture"),
        "--dependency",
        &first,
        "--dependency",
        &second,
        "--out-dir",
        output.to_str().expect("UTF-8 output"),
    ]);

    assert!(!generated, "two directories for one dependency were merged");
}
