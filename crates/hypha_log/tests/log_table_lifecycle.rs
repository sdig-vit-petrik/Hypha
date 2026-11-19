//! The process holds one logging configuration; each test starts from the default.
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

mod support;

use hypha_log::{Level, LogError, commit, enabled, prepare, register_module};
use support::{TestResult, in_own_process, log_table, parameter_file, temp_dir};

#[test]
fn the_default_configuration_is_console_at_debug() -> TestResult {
    in_own_process(
        "the_default_configuration_is_console_at_debug",
        the_default_configuration_is_console_at_debug_body,
    )
}

fn the_default_configuration_is_console_at_debug_body() -> TestResult {
    let module = register_module("lifecycle_default")?;
    assert!(enabled(module, Level::Info));
    assert!(enabled(module, Level::Error));
    assert!(enabled(module, Level::Debug));
    assert!(!enabled(module, Level::Trace));
    Ok(())
}

#[test]
fn the_first_table_applies() -> TestResult {
    in_own_process("the_first_table_applies", the_first_table_applies_body)
}

fn the_first_table_applies_body() -> TestResult {
    let dir = temp_dir("lifecycle_first")?;
    let module = register_module("lifecycle_first")?;
    let prepared = prepare(
        &log_table("[log]\nlevel = \"trace\"\nconsole_level = \"trace\"\n")?,
        &parameter_file(&dir),
    )
    .map_err(|error| format!("{error:?}"))?;
    assert!(!enabled(module, Level::Trace), "prepare must not change the configuration");
    commit(prepared).expect("commit");
    assert!(enabled(module, Level::Trace));
    Ok(())
}

#[test]
fn an_equal_table_is_accepted_without_opening_files() -> TestResult {
    in_own_process(
        "an_equal_table_is_accepted_without_opening_files",
        an_equal_table_is_accepted_without_opening_files_body,
    )
}

fn an_equal_table_is_accepted_without_opening_files_body() -> TestResult {
    let dir = temp_dir("lifecycle_equal")?;
    let file = parameter_file(&dir);
    let sink = "[log]\nlevel = \"info\"\n[log.sink.a]\npath = \"a.log\"\n";
    let first = prepare(&log_table(sink)?, &file).map_err(|error| format!("{error:?}"))?;
    commit(first).expect("commit");
    let again = prepare(&log_table(sink)?, &file).map_err(|error| format!("{error:?}"))?;
    commit(again).expect("commit");
    let spelled_with_defaults = "[log]\nlevel = \"info\"\nconsole = true\nconsole_level = \"trace\"\n\
        [log.sink.a]\npath = \"./a.log\"\nlevel = \"info\"\nrotation_mb = 5\nrotation_count = 3\n";
    let defaults = prepare(&log_table(spelled_with_defaults)?, &file);
    assert!(defaults.is_ok(), "an equal resolved configuration must be accepted");
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore = "the console reopens fd 2 non-blocking, which Miri cannot")]
fn a_different_table_is_invalid_state() -> TestResult {
    in_own_process("a_different_table_is_invalid_state", a_different_table_is_invalid_state_body)
}

fn a_different_table_is_invalid_state_body() -> TestResult {
    let dir = temp_dir("lifecycle_different")?;
    let file = parameter_file(&dir);
    let first =
        prepare(&log_table("[log]\nlevel = \"debug\"\nconsole_level = \"debug\"\n")?, &file)
            .map_err(|error| format!("{error:?}"))?;
    commit(first).expect("commit");
    let other = prepare(&log_table("[log]\nlevel = \"warn\"\n")?, &file);
    assert!(matches!(other, Err(LogError::InvalidState(_))));
    let module = register_module("lifecycle_different")?;
    assert!(enabled(module, Level::Debug), "the first configuration stays");
    Ok(())
}

#[test]
fn a_relative_path_resolves_against_the_parameter_file_directory() -> TestResult {
    in_own_process(
        "a_relative_path_resolves_against_the_parameter_file_directory",
        a_relative_path_resolves_against_the_parameter_file_directory_body,
    )
}

fn a_relative_path_resolves_against_the_parameter_file_directory_body() -> TestResult {
    let dir = temp_dir("lifecycle_relative")?;
    let source = "[log]\nconsole = false\n[log.sink.a]\npath = \"nested/rel.log\"\n";
    let prepared = prepare(&log_table(source)?, &parameter_file(&dir))
        .map_err(|error| format!("{error:?}"))?;
    commit(prepared).expect("commit");
    let module = register_module("lifecycle_relative")?;
    hypha_log::write(module, Level::Info, b"relative path line");
    let content = std::fs::read_to_string(dir.join("nested/rel.log"))?;
    assert!(content.contains("relative path line"));
    Ok(())
}

#[test]
fn a_dropped_prepared_config_changes_nothing_and_unlocks_its_files() -> TestResult {
    in_own_process(
        "a_dropped_prepared_config_changes_nothing_and_unlocks_its_files",
        a_dropped_prepared_config_changes_nothing_and_unlocks_its_files_body,
    )
}

fn a_dropped_prepared_config_changes_nothing_and_unlocks_its_files_body() -> TestResult {
    let dir = temp_dir("lifecycle_drop")?;
    let file = parameter_file(&dir);
    let source = "[log]\nlevel = \"trace\"\n[log.sink.a]\npath = \"a.log\"\n";
    let discarded = prepare(&log_table(source)?, &file).map_err(|error| format!("{error:?}"))?;
    drop(discarded);
    let module = register_module("lifecycle_drop")?;
    assert!(enabled(module, Level::Debug));
    assert!(!enabled(module, Level::Trace));
    let other = prepare(&log_table("[log.sink.a]\npath = \"a.log\"\nlevel = \"warn\"\n")?, &file);
    assert!(other.is_ok(), "the file must be unlocked after the drop");
    Ok(())
}
