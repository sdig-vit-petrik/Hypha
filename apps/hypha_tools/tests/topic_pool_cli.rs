//! `hypha topic pool` answers from the rule of the transport and needs no bus (issue 241).
#![allow(clippy::expect_used)]

use std::process::Command;

fn run(arguments: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_hypha"))
        .args(["topic", "pool"])
        .args(arguments)
        .output()
        .expect("run hypha topic pool");
    let text = |bytes: Vec<u8>| String::from_utf8_lossy(&bytes).into_owned();
    (output.status.code(), text(output.stdout), text(output.stderr))
}

#[test]
fn it_gives_the_pool_of_a_5_mp_mono8_camera_by_the_documented_rule() {
    let (code, stdout, stderr) =
        run(&["--payload", "5000048", "--publishers", "1", "--subscribers", "2", "--depth", "2"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains("23 samples of 5000152 bytes"), "{stdout}");
    assert!(stdout.contains("= 115003496 bytes (109.7 MiB)"), "{stdout}");
}

#[test]
fn a_limit_it_does_not_give_is_that_of_the_default_policy() {
    let (code, stdout, stderr) = run(&["--payload", "1800000"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains("649 samples"), "{stdout}");
    assert!(stdout.contains("1.09 GiB"), "{stdout}");
}

#[test]
fn a_depth_of_zero_is_refused() {
    let (code, stdout, stderr) = run(&["--payload", "10", "--depth", "0"]);
    assert_eq!(code, Some(1), "{stdout}");
    assert!(stderr.contains("max_depth"), "{stderr}");
}
