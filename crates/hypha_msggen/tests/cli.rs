//! End-to-end generator command tests.
#![expect(clippy::expect_used, reason = "a failed test setup stops the test with its message")]

use std::path::PathBuf;
use std::process::Command;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo_msgs")
}

fn output_dir() -> PathBuf {
    std::env::temp_dir().join(format!("hypha_msggen_cli_{}", std::process::id()))
}

#[test]
fn rust_cli_emits_modules_for_messages_and_services() {
    let output = output_dir();
    if output.exists() {
        std::fs::remove_dir_all(&output).expect("remove stale generator output");
    }
    let fixture = fixture_dir();
    let status = Command::new(env!("CARGO_BIN_EXE_hypha_msggen"))
        .args(["--package", "demo_msgs"])
        .args(["--msg-dir", fixture.to_str().expect("UTF-8 fixture")])
        .args(["--srv-dir", fixture.to_str().expect("UTF-8 fixture")])
        .args(["--out-dir", output.to_str().expect("UTF-8 output")])
        .args(["--language", "rust"])
        .status()
        .expect("run hypha_msggen");
    assert!(status.success());

    let package = output.join("demo_msgs");
    assert!(package.join("gps_fix.rs").is_file());
    assert!(package.join("gps_stamped.rs").is_file());
    assert!(package.join("add_two_ints_request.rs").is_file());
    assert!(package.join("add_two_ints_response.rs").is_file());
    assert!(package.join("add_two_ints.rs").is_file());
    std::fs::remove_dir_all(output).expect("remove generator output");
}

#[test]
fn hypha_msggen_refuses_a_field_name_that_is_not_an_identifier_with_its_file_and_line() {
    let cases = [
        (
            "float64 x 1.0",
            "line 2: unexpected `1.0` after field `x`; a default value is ROS 2 syntax, which is not supported",
        ),
        (
            "int32 a-b",
            "line 2: field or constant name `a-b` is not a letter followed by letters, digits or `_`",
        ),
        (
            "int32 2x",
            "line 2: field or constant name `2x` is not a letter followed by letters, digits or `_`",
        ),
        (
            "int32 x y",
            "line 2: unexpected `y` after field `x`; a default value is ROS 2 syntax, which is not supported",
        ),
    ];
    let root = std::env::temp_dir().join(format!("hypha_msggen_names_{}", std::process::id()));
    for (language, (declaration, reason)) in
        ["cpp", "rust"].into_iter().flat_map(|language| cases.map(|case| (language, case)))
    {
        let messages = root.join("msg");
        std::fs::create_dir_all(&messages).expect("create message directory");
        std::fs::write(messages.join("Bad.msg"), format!("int32 a\n{declaration}\n"))
            .expect("write message");
        let output = Command::new(env!("CARGO_BIN_EXE_hypha_msggen"))
            .args(["--package", "names_msgs"])
            .args(["--msg-dir", messages.to_str().expect("UTF-8 path")])
            .args(["--out-dir", root.join("out").to_str().expect("UTF-8 path")])
            .args(["--language", language])
            .output()
            .expect("run hypha_msggen");
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        std::fs::remove_dir_all(&root).expect("remove test directory");
        assert!(!output.status.success(), "{language} `{declaration}` generated");
        assert!(stderr.contains("Bad.msg"), "stderr: {stderr}");
        assert!(stderr.contains(reason), "stderr: {stderr}");
    }
}

#[test]
fn hypha_msggen_refuses_an_out_of_range_constant_with_its_line() {
    let root = std::env::temp_dir().join(format!("hypha_msggen_c6_{}", std::process::id()));
    let messages = root.join("msg");
    std::fs::create_dir_all(&messages).expect("create message directory");
    std::fs::write(messages.join("Bad.msg"), "int32 a\nbyte X=200\n").expect("write message");
    let output = Command::new(env!("CARGO_BIN_EXE_hypha_msggen"))
        .args(["--package", "c6_msgs"])
        .args(["--msg-dir", messages.to_str().expect("UTF-8 path")])
        .args(["--out-dir", root.join("out").to_str().expect("UTF-8 path")])
        .args(["--language", "cpp"])
        .output()
        .expect("run hypha_msggen");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    std::fs::remove_dir_all(&root).expect("remove test directory");
    assert!(!output.status.success());
    assert!(stderr.contains("Bad.msg"), "stderr: {stderr}");
    assert!(
        stderr.contains("line 2: constant value 200 is out of range for byte (-128 to 127)"),
        "stderr: {stderr}"
    );
}
