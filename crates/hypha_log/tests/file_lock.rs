//! A sink file is locked while a process holds it.
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

mod support;

use hypha_log::{LogError, prepare};
use std::fs::OpenOptions;
use support::{TestResult, in_own_process, log_table, parameter_file, temp_dir};

const SINK: &str = "[log]\nconsole = false\n[log.sink.a]\npath = \"locked.log\"\n";

#[test]
#[cfg_attr(miri, ignore = "the console reopens fd 2 non-blocking, which Miri cannot")]
fn a_second_prepare_of_a_locked_file_is_invalid_state() -> TestResult {
    in_own_process(
        "a_second_prepare_of_a_locked_file_is_invalid_state",
        a_second_prepare_of_a_locked_file_is_invalid_state_body,
    )
}

fn a_second_prepare_of_a_locked_file_is_invalid_state_body() -> TestResult {
    let dir = temp_dir("lock_second")?;
    let file = parameter_file(&dir);
    let first = prepare(&log_table(SINK)?, &file).map_err(|error| format!("{error:?}"))?;
    let second = prepare(&log_table(SINK)?, &file);
    assert!(matches!(second, Err(LogError::InvalidState(_))));
    drop(first);
    let third = prepare(&log_table(SINK)?, &file);
    assert!(third.is_ok(), "the lock must be released when the prepared config drops");
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore = "the console reopens fd 2 non-blocking, which Miri cannot")]
fn a_file_locked_by_another_holder_is_invalid_state() -> TestResult {
    in_own_process(
        "a_file_locked_by_another_holder_is_invalid_state",
        a_file_locked_by_another_holder_is_invalid_state_body,
    )
}

fn a_file_locked_by_another_holder_is_invalid_state_body() -> TestResult {
    let dir = temp_dir("lock_external")?;
    // The lock lives on `<path>.lock`, not on the log file itself.
    let holder = OpenOptions::new().create(true).append(true).open(dir.join("locked.log.lock"))?;
    holder.lock()?;
    let result = prepare(&log_table(SINK)?, &parameter_file(&dir));
    let Err(LogError::InvalidState(message)) = result else {
        return Err("expected InvalidState".into());
    };
    assert!(message.contains("locked.log"));
    Ok(())
}

#[test]
fn a_disabled_sink_does_not_open_or_lock_its_file() -> TestResult {
    in_own_process(
        "a_disabled_sink_does_not_open_or_lock_its_file",
        a_disabled_sink_does_not_open_or_lock_its_file_body,
    )
}

fn a_disabled_sink_does_not_open_or_lock_its_file_body() -> TestResult {
    let dir = temp_dir("lock_disabled")?;
    let source = "[log.sink.a]\npath = \"never.log\"\nenabled = false\n";
    let prepared = prepare(&log_table(source)?, &parameter_file(&dir));
    assert!(prepared.is_ok());
    assert!(!dir.join("never.log").exists());
    Ok(())
}
