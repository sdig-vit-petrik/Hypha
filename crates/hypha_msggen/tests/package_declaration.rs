//! `--package`, `--msg-dir`, `--srv-dir` and `--dependency <package>=<dir>`; the
//! directory names carry no meaning.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_dir(package: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dependency").join(package)
}

fn scratch_dir(case: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("hypha_msggen_package_{case}_{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("remove stale scratch directory");
    }
    std::fs::create_dir_all(&dir).expect("create scratch directory");
    dir
}

fn utf8(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

fn run_generator(args: &[&str]) -> bool {
    Command::new(env!("CARGO_BIN_EXE_hypha_msggen"))
        .args(args)
        .status()
        .expect("run hypha_msggen")
        .success()
}

#[test]
fn the_package_name_comes_from_the_flag_and_not_from_the_directory() {
    let scratch = scratch_dir("flag");
    let out = scratch.join("out");
    let app = fixture_dir("app_msgs");
    let base = fixture_dir("base_msgs");
    let dependency = format!("base_msgs={}", utf8(&base));

    let generated = run_generator(&[
        "--package",
        "renamed_msgs",
        "--msg-dir",
        utf8(&app),
        "--dependency",
        &dependency,
        "--out-dir",
        utf8(&out),
    ]);

    assert!(generated, "generator refused a declared package");
    assert!(out.join("renamed_msgs/Reading.h").is_file(), "header missing under the flag's name");
    assert!(!out.join("app_msgs").exists(), "the directory name was used as a package name");
    std::fs::remove_dir_all(scratch).expect("remove scratch directory");
}

#[test]
fn a_dependency_resolves_under_the_name_the_flag_gives_it() {
    let scratch = scratch_dir("dependency_name");
    let out = scratch.join("out");
    let app = fixture_dir("app_msgs");
    let base = fixture_dir("base_msgs");
    let dependency = format!("base_msgs={}", utf8(&base));

    let generated = run_generator(&[
        "--package",
        "app_msgs",
        "--msg-dir",
        utf8(&app),
        "--dependency",
        &dependency,
        "--out-dir",
        utf8(&out),
    ]);

    assert!(generated);
    let header = std::fs::read_to_string(out.join("app_msgs/Reading.h")).expect("read header");
    assert!(header.contains("base_msgs::Stamp stamp"));
    assert!(!out.join("base_msgs").exists(), "a dependency was emitted");
    std::fs::remove_dir_all(scratch).expect("remove scratch directory");
}

#[test]
fn a_missing_package_flag_refuses() {
    let scratch = scratch_dir("no_package");
    let base = fixture_dir("base_msgs");

    let generated =
        run_generator(&["--msg-dir", utf8(&base), "--out-dir", utf8(&scratch.join("out"))]);

    assert!(!generated, "generator guessed a package name");
    std::fs::remove_dir_all(scratch).expect("remove scratch directory");
}

#[test]
fn a_dependency_without_a_package_name_refuses() {
    let scratch = scratch_dir("bare_dependency");
    let app = fixture_dir("app_msgs");
    let base = fixture_dir("base_msgs");

    let generated = run_generator(&[
        "--package",
        "app_msgs",
        "--msg-dir",
        utf8(&app),
        "--dependency",
        utf8(&base),
        "--out-dir",
        utf8(&scratch.join("out")),
    ]);

    assert!(!generated, "generator accepted a dependency with no package name");
    std::fs::remove_dir_all(scratch).expect("remove scratch directory");
}

#[test]
fn a_dependency_with_the_name_of_the_owned_package_refuses() {
    let scratch = scratch_dir("same_name");
    let app = fixture_dir("app_msgs");
    let base = fixture_dir("base_msgs");
    let dependency = format!("app_msgs={}", utf8(&base));

    let generated = run_generator(&[
        "--package",
        "app_msgs",
        "--msg-dir",
        utf8(&app),
        "--dependency",
        &dependency,
        "--out-dir",
        utf8(&scratch.join("out")),
    ]);

    assert!(!generated, "one package name was owned and read at once");
    std::fs::remove_dir_all(scratch).expect("remove scratch directory");
}

#[test]
fn a_definition_directory_is_read_flat() {
    let scratch = scratch_dir("flat");
    let msgs = scratch.join("flat_msgs");
    std::fs::create_dir_all(msgs.join("nested")).expect("create nested directory");
    std::fs::write(msgs.join("Top.msg"), "int32 value\n").expect("write top message");
    std::fs::write(msgs.join("nested/Inner.msg"), "int32 value\n").expect("write nested message");
    let out = scratch.join("out");

    let generated = run_generator(&[
        "--package",
        "flat_msgs",
        "--msg-dir",
        utf8(&msgs),
        "--out-dir",
        utf8(&out),
    ]);

    assert!(generated);
    assert!(out.join("flat_msgs/Top.h").is_file());
    assert!(!out.join("flat_msgs/Inner.h").exists(), "a subdirectory was read as definitions");
    std::fs::remove_dir_all(scratch).expect("remove scratch directory");
}

#[test]
fn a_definition_directory_that_does_not_exist_refuses() {
    let scratch = scratch_dir("missing_dir");

    let generated = run_generator(&[
        "--package",
        "ghost_msgs",
        "--msg-dir",
        utf8(&scratch.join("absent")),
        "--out-dir",
        utf8(&scratch.join("out")),
    ]);

    assert!(!generated);
    std::fs::remove_dir_all(scratch).expect("remove scratch directory");
}

#[test]
fn a_package_with_no_definitions_refuses() {
    let scratch = scratch_dir("empty");
    let empty = scratch.join("empty_msgs");
    std::fs::create_dir_all(&empty).expect("create empty package");

    let generated = run_generator(&[
        "--package",
        "empty_msgs",
        "--msg-dir",
        utf8(&empty),
        "--out-dir",
        utf8(&scratch.join("out")),
    ]);

    assert!(!generated);
    std::fs::remove_dir_all(scratch).expect("remove scratch directory");
}
