//! Refusal of an invalid `[log]` table, and acceptance of the valid bounds.
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

mod support;

use hypha_log::{LogError, prepare};
use support::{TestResult, in_own_process, log_table, parameter_file, temp_dir};

const REFUSED: &[(&str, &str, &str)] = &[
    ("unknown table", "[log.bogus]\nx = 1\n", "bogus"),
    ("unknown key", "[log]\nfoo = 1\n", "foo"),
    ("removed key async", "[log]\nasync = true\n", "async"),
    ("removed key overflow_policy", "[log]\noverflow_policy = \"block\"\n", "overflow_policy"),
    ("level wrong type", "[log]\nlevel = 5\n", "level"),
    ("alias err", "[log]\nlevel = \"err\"\n", "level"),
    ("alias critical", "[log]\nlevel = \"critical\"\n", "level"),
    ("unknown console level", "[log]\nconsole_level = \"loud\"\n", "console_level"),
    ("console wrong type", "[log]\nconsole = \"yes\"\n", "console"),
    ("sink is not a table", "[log]\nsink = 5\n", "sink"),
    ("module is not a table", "[log]\nmodule = 5\n", "module"),
    ("sink without path", "[log.sink.a]\nlevel = \"info\"\n", "path"),
    ("sink empty path", "[log.sink.a]\npath = \"\"\n", "path"),
    ("sink path wrong type", "[log.sink.a]\npath = 5\n", "path"),
    ("sink unknown key", "[log.sink.a]\npath = \"x.log\"\ncolour = true\n", "colour"),
    ("sink enabled wrong type", "[log.sink.a]\npath = \"x.log\"\nenabled = 1\n", "enabled"),
    ("sink unknown level", "[log.sink.a]\npath = \"x.log\"\nlevel = \"x\"\n", "level"),
    ("rotation_mb zero", "[log.sink.a]\npath = \"x.log\"\nrotation_mb = 0\n", "rotation_mb"),
    ("rotation_mb 1025", "[log.sink.a]\npath = \"x.log\"\nrotation_mb = 1025\n", "rotation_mb"),
    ("rotation_mb string", "[log.sink.a]\npath = \"x.log\"\nrotation_mb = \"5\"\n", "rotation_mb"),
    ("rotation_mb float", "[log.sink.a]\npath = \"x.log\"\nrotation_mb = 5.0\n", "rotation_mb"),
    (
        "rotation_count zero",
        "[log.sink.a]\npath = \"x.log\"\nrotation_count = 0\n",
        "rotation_count",
    ),
    (
        "rotation_count 101",
        "[log.sink.a]\npath = \"x.log\"\nrotation_count = 101\n",
        "rotation_count",
    ),
    ("module unknown key", "[log.module.m]\nfoo = 1\n", "foo"),
    ("module unknown level", "[log.module.m]\nlevel = \"x\"\n", "level"),
    ("module enabled wrong type", "[log.module.m]\nenabled = \"no\"\n", "enabled"),
    ("module name with space", "[log.module.\"bad name\"]\nlevel = \"info\"\n", "bad name"),
    ("module name empty", "[log.module.\"\"]\nlevel = \"info\"\n", "module"),
    (
        "module name too long",
        "[log.module.\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"]\nlevel = \"info\"\n",
        "aaaaaaaa",
    ),
    (
        "duplicate path",
        "[log.sink.a]\npath = \"same.log\"\n[log.sink.b]\npath = \"./same.log\"\n",
        "same.log",
    ),
];

#[test]
#[cfg_attr(miri, ignore = "the console reopens fd 2 non-blocking, which Miri cannot")]
fn every_invalid_table_is_refused_with_file_table_and_key() -> TestResult {
    in_own_process(
        "every_invalid_table_is_refused_with_file_table_and_key",
        every_invalid_table_is_refused_with_file_table_and_key_body,
    )
}

fn every_invalid_table_is_refused_with_file_table_and_key_body() -> TestResult {
    let dir = temp_dir("validation_refused")?;
    let file = parameter_file(&dir);
    for (case, source, key) in REFUSED {
        let table = log_table(source)?;
        let result = prepare(&table, &file);
        let Err(LogError::InvalidArgument(message)) = result else {
            return Err(format!("{case}: expected InvalidArgument").into());
        };
        let names_file = message.contains(&file.display().to_string());
        assert!(names_file, "{case}: message {message:?} names no file");
        assert!(message.contains(key), "{case}: message {message:?} names no key {key}");
        assert!(message.contains("log"), "{case}: message {message:?} names no table");
    }
    Ok(())
}

#[test]
fn the_bounds_of_rotation_are_accepted() -> TestResult {
    in_own_process("the_bounds_of_rotation_are_accepted", the_bounds_of_rotation_are_accepted_body)
}

fn the_bounds_of_rotation_are_accepted_body() -> TestResult {
    let dir = temp_dir("validation_bounds")?;
    let file = parameter_file(&dir);
    for (mb, count) in [(1, 1), (1024, 100), (5, 3)] {
        let source = format!(
            "[log.sink.a]\npath = \"a.log\"\nrotation_mb = {mb}\nrotation_count = {count}\n"
        );
        prepare(&log_table(&source)?, &file).map_err(|error| format!("{error:?}"))?;
    }
    Ok(())
}

#[test]
fn every_level_name_is_accepted_and_a_disabled_sink_may_share_a_path() -> TestResult {
    in_own_process(
        "every_level_name_is_accepted_and_a_disabled_sink_may_share_a_path",
        every_level_name_is_accepted_and_a_disabled_sink_may_share_a_path_body,
    )
}

fn every_level_name_is_accepted_and_a_disabled_sink_may_share_a_path_body() -> TestResult {
    let dir = temp_dir("validation_levels")?;
    let file = parameter_file(&dir);
    for name in ["trace", "debug", "info", "warn", "error", "fatal", "off"] {
        let source = format!("[log]\nlevel = \"{name}\"\nconsole_level = \"{name}\"\n");
        prepare(&log_table(&source)?, &file).map_err(|error| format!("{error:?}"))?;
    }
    let shared =
        "[log.sink.a]\npath = \"s.log\"\n[log.sink.b]\npath = \"s.log\"\nenabled = false\n";
    prepare(&log_table(shared)?, &file).map_err(|error| format!("{error:?}"))?;
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore = "the console reopens fd 2 non-blocking, which Miri cannot")]
fn a_refused_table_changes_nothing() -> TestResult {
    in_own_process("a_refused_table_changes_nothing", a_refused_table_changes_nothing_body)
}

fn a_refused_table_changes_nothing_body() -> TestResult {
    let dir = temp_dir("validation_nothing_changes")?;
    let file = parameter_file(&dir);
    let module = hypha_log::register_module("probe")?;
    assert!(hypha_log::enabled(module, hypha_log::Level::Debug));
    assert!(!hypha_log::enabled(module, hypha_log::Level::Trace));
    let refused = log_table("[log]\nlevel = \"trace\"\nbogus = 1\n")?;
    assert!(prepare(&refused, &file).is_err());
    assert!(hypha_log::enabled(module, hypha_log::Level::Debug));
    assert!(!hypha_log::enabled(module, hypha_log::Level::Trace));
    let valid = log_table("[log]\nlevel = \"trace\"\nconsole_level = \"trace\"\n")?;
    let prepared = prepare(&valid, &file).map_err(|error| format!("{error:?}"))?;
    hypha_log::commit(prepared).expect("commit");
    assert!(hypha_log::enabled(module, hypha_log::Level::Debug));
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore = "the console reopens fd 2 non-blocking, which Miri cannot")]
fn an_unwritable_parent_directory_is_refused() -> TestResult {
    in_own_process(
        "an_unwritable_parent_directory_is_refused",
        an_unwritable_parent_directory_is_refused_body,
    )
}

fn an_unwritable_parent_directory_is_refused_body() -> TestResult {
    let dir = temp_dir("validation_parent")?;
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"file")?;
    let source = format!("[log.sink.a]\npath = \"{}\"\n", blocker.join("deep/x.log").display());
    let result = prepare(&log_table(&source)?, &parameter_file(&dir));
    assert!(matches!(result, Err(LogError::InvalidArgument(_))));
    Ok(())
}
