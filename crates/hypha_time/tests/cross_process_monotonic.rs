//! Cross-process monotonic clock origin test.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

use std::process::Command;

use hypha_time::steady_now;

const CHILD_MODE: &str = "HYPHA_CLOCK_CHILD_READING";
const CHILD_MARKER: &str = "HYPHA_CLOCK_READING=";

#[test]
#[cfg_attr(miri, ignore = "spawns a second process, which Miri cannot")]
fn monotonic_readings_share_a_cross_process_origin() {
    if std::env::var_os(CHILD_MODE).is_some() {
        println!("{CHILD_MARKER}{}", steady_now().nanos());
        return;
    }

    let before = steady_now().nanos();
    let output = Command::new(std::env::current_exe().expect("current test executable"))
        .args(["--exact", "monotonic_readings_share_a_cross_process_origin_child", "--nocapture"])
        .env(CHILD_MODE, "1")
        .output()
        .expect("spawn clock child");
    assert!(output.status.success(), "clock child failed: {:?}", output.status);
    let after = steady_now().nanos();
    let stdout = String::from_utf8(output.stdout).expect("child output is UTF-8");
    let line = stdout.lines().find(|line| line.starts_with(CHILD_MARKER)).expect("child reading");
    let child = line[CHILD_MARKER.len()..].parse::<i64>().expect("child reading is numeric");
    assert!(
        child >= before && child <= after,
        "child clock is not comparable: {before} <= {child} <= {after}"
    );
}

#[test]
fn monotonic_readings_share_a_cross_process_origin_child() {
    if std::env::var_os(CHILD_MODE).is_some() {
        println!("{CHILD_MARKER}{}", steady_now().nanos());
    }
}
