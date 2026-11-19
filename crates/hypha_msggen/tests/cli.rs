//! End-to-end generator command tests.

use std::path::PathBuf;
use std::process::Command;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/demo_msgs")
}

fn output_dir() -> PathBuf {
    std::env::temp_dir().join(format!("hypha_msggen_cli_{}", std::process::id()))
}

#[test]
#[allow(clippy::expect_used)]
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
#[allow(clippy::expect_used)]
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
