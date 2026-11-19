//! Module registration and level selection.
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

mod support;

use hypha_log::{Level, LogError, ModuleId};
use hypha_log::{
    commit, enabled, health, module_or_default, prepare, register_module, report, write,
};
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use support::{TestResult, in_own_process, log_table, parameter_file, temp_dir};

#[test]
fn level_names_and_raw_values_convert() -> TestResult {
    in_own_process("level_names_and_raw_values_convert", level_names_and_raw_values_convert_body)
}

fn level_names_and_raw_values_convert_body() {
    let names = ["trace", "debug", "info", "warn", "error", "fatal", "off"];
    for (raw, name) in (0_u32..).zip(names) {
        assert_eq!(Level::from_name(name).map(|level| level as u32), Some(raw));
        assert_eq!(Level::from_raw(raw).map(|level| level as u32), Some(raw));
    }
    for rejected in ["err", "critical", "INFO", "", "warning"] {
        assert_eq!(Level::from_name(rejected), None);
    }
    assert_eq!(Level::from_raw(7), None);
    assert!(Level::Trace < Level::Info && Level::Fatal < Level::Off);
}

#[test]
fn a_module_name_follows_the_name_rule() -> TestResult {
    in_own_process("a_module_name_follows_the_name_rule", a_module_name_follows_the_name_rule_body)
}

fn a_module_name_follows_the_name_rule_body() {
    let sixty_four = "a".repeat(64);
    for valid in ["a", "a/b_c9", "AZ", sixty_four.as_str()] {
        assert!(register_module(valid).is_ok(), "{valid} must be valid");
    }
    let sixty_five = "a".repeat(65);
    for invalid in ["", sixty_five.as_str(), "a b", "a-b", "a.b", "caf\u{e9}", "a\0b"] {
        let result = register_module(invalid);
        assert!(matches!(result, Err(LogError::InvalidArgument(_))), "{invalid:?}");
    }
}

#[test]
fn the_same_name_gives_the_same_id_and_default_exists() -> TestResult {
    in_own_process(
        "the_same_name_gives_the_same_id_and_default_exists",
        the_same_name_gives_the_same_id_and_default_exists_body,
    )
}

fn the_same_name_gives_the_same_id_and_default_exists_body() -> TestResult {
    assert_eq!(register_module("default")?, ModuleId(0));
    assert_eq!(register_module("twin")?, register_module("twin")?);
    assert_ne!(register_module("one")?, register_module("two")?);
    Ok(())
}

#[test]
fn the_257th_module_is_a_resource_limit() -> TestResult {
    in_own_process(
        "the_257th_module_is_a_resource_limit",
        the_257th_module_is_a_resource_limit_body,
    )
}

fn the_257th_module_is_a_resource_limit_body() -> TestResult {
    let mut registered = 1_u32; // "default"
    for index in 0..300_u32 {
        let name = format!("limit_{index}");
        match register_module(&name) {
            Ok(_) => registered = registered.wrapping_add(1),
            Err(error) => assert_eq!(error, LogError::ResourceLimit),
        }
    }
    assert_eq!(registered, 256);
    assert!(matches!(register_module("one_more"), Err(LogError::ResourceLimit)));
    assert_eq!(register_module("limit_0")?, register_module("limit_0")?);
    Ok(())
}

#[test]
fn an_unknown_module_is_disabled_and_a_write_to_it_is_rejected() -> TestResult {
    in_own_process(
        "an_unknown_module_is_disabled_and_a_write_to_it_is_rejected",
        an_unknown_module_is_disabled_and_a_write_to_it_is_rejected_body,
    )
}

fn an_unknown_module_is_disabled_and_a_write_to_it_is_rejected_body() {
    assert!(!enabled(ModuleId(4000), Level::Fatal));
    let before = health().rejected_lines;
    write(ModuleId(4000), Level::Fatal, b"nobody");
    assert_eq!(health().rejected_lines, before.wrapping_add(1));
}

#[test]
fn level_off_is_never_enabled_and_a_write_at_off_is_rejected() -> TestResult {
    in_own_process(
        "level_off_is_never_enabled_and_a_write_at_off_is_rejected",
        level_off_is_never_enabled_and_a_write_at_off_is_rejected_body,
    )
}

fn level_off_is_never_enabled_and_a_write_at_off_is_rejected_body() -> TestResult {
    let module = register_module("off_probe")?;
    assert!(!enabled(module, Level::Off));
    let before = health().rejected_lines;
    write(module, Level::Off, b"invalid level");
    assert_eq!(health().rejected_lines, before.wrapping_add(1));
    hypha_log::count_rejected_line();
    assert_eq!(health().rejected_lines, before.wrapping_add(2));
    Ok(())
}

#[test]
fn module_level_and_sink_level_select_the_targets() -> TestResult {
    in_own_process(
        "module_level_and_sink_level_select_the_targets",
        module_level_and_sink_level_select_the_targets_body,
    )
}

fn module_level_and_sink_level_select_the_targets_body() -> TestResult {
    let dir = temp_dir("levels_select")?;
    let source = "[log]\nlevel = \"warn\"\nconsole = false\n\
        [log.sink.low]\npath = \"low.log\"\nlevel = \"debug\"\n\
        [log.sink.high]\npath = \"high.log\"\nlevel = \"error\"\n\
        [log.module.noisy]\nlevel = \"trace\"\n\
        [log.module.mid]\nlevel = \"info\"\n\
        [log.module.quiet]\nenabled = false\n\
        [log.module.quiet_level]\nlevel = \"off\"\n";
    let early = register_module("mid")?;
    let prepared = prepare(&log_table(source)?, &parameter_file(&dir))
        .map_err(|error| format!("{error:?}"))?;
    commit(prepared).expect("commit");
    let noisy = register_module("noisy")?;
    let quiet = register_module("quiet")?;
    let quiet_level = register_module("quiet_level")?;
    let fallback = register_module("fallback")?;

    assert!(enabled(early, Level::Info) && !enabled(early, Level::Debug));
    assert!(enabled(noisy, Level::Debug), "debug reaches the low sink");
    assert!(!enabled(noisy, Level::Trace), "no target takes trace");
    for level in [Level::Debug, Level::Error, Level::Fatal] {
        assert!(!enabled(quiet, level) && !enabled(quiet_level, level));
    }
    assert!(enabled(fallback, Level::Warn) && !enabled(fallback, Level::Info));

    write(early, Level::Info, b"mid info");
    write(fallback, Level::Warn, b"fallback warn");
    write(fallback, Level::Error, b"fallback error");
    write(noisy, Level::Trace, b"noisy trace");
    write(quiet, Level::Fatal, b"quiet fatal");
    let low = std::fs::read_to_string(dir.join("low.log"))?;
    let high = std::fs::read_to_string(dir.join("high.log"))?;
    assert!(low.contains("mid info") && low.contains("fallback warn"));
    assert!(low.contains("fallback error") && !low.contains("noisy trace"));
    assert!(!low.contains("quiet fatal"));
    assert!(high.contains("fallback error"));
    assert!(!high.contains("fallback warn") && !high.contains("mid info"));
    assert_eq!(health().lines_written, 3);
    Ok(())
}

struct Probe(&'static AtomicBool);

impl Display for Probe {
    fn fmt(&self, out: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.store(true, Ordering::SeqCst);
        out.write_str("probe")
    }
}

#[test]
#[cfg_attr(miri, ignore = "the console reopens fd 2 non-blocking, which Miri cannot")]
fn report_formats_only_when_the_level_is_enabled() -> TestResult {
    in_own_process(
        "report_formats_only_when_the_level_is_enabled",
        report_formats_only_when_the_level_is_enabled_body,
    )
}

fn report_formats_only_when_the_level_is_enabled_body() {
    static FORMATTED: AtomicBool = AtomicBool::new(false);
    report("report_probe", Level::Trace, format_args!("{}", Probe(&FORMATTED)));
    assert!(!FORMATTED.load(Ordering::SeqCst), "a disabled line must not format");
    report("report_probe", Level::Error, format_args!("{}", Probe(&FORMATTED)));
    assert!(FORMATTED.load(Ordering::SeqCst));
    report("bad name", Level::Error, format_args!("x"));
}

/// Fill the table: `default` and 255 more names.
fn fill_the_module_table() -> TestResult {
    for index in 0..255_u32 {
        register_module(&format!("fill_{index}"))?;
    }
    assert!(matches!(register_module("one_more"), Err(LogError::ResourceLimit)));
    Ok(())
}

/// A file sink at `trace`, with `default` at `level`.
fn configure_default_level(
    tag: &str,
    level: &str,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let dir = temp_dir(tag)?;
    let source = format!(
        "[log]\nconsole = false\nlevel = \"{level}\"\n[log.sink.main]\npath = \"m.log\"\nlevel = \"trace\"\n"
    );
    let prepared = prepare(&log_table(&source)?, &parameter_file(&dir))
        .map_err(|error| format!("{error:?}"))?;
    commit(prepared).expect("commit");
    Ok(dir.join("m.log"))
}

#[test]
fn a_name_past_256_modules_writes_under_the_default_level() -> TestResult {
    in_own_process(
        "a_name_past_256_modules_writes_under_the_default_level",
        a_name_past_256_modules_writes_under_the_default_level_body,
    )
}

fn a_name_past_256_modules_writes_under_the_default_level_body() -> TestResult {
    let file = configure_default_level("overflow_report", "warn")?;
    fill_the_module_table()?;
    assert_eq!(module_or_default("late_name")?, ModuleId(0));
    assert!(matches!(module_or_default("bad name"), Err(LogError::InvalidArgument(_))));
    report("late_name", Level::Info, format_args!("below the default level"));
    report("late_name", Level::Warn, format_args!("at the default level"));
    let text = std::fs::read_to_string(file)?;
    assert!(!text.contains("below the default level"), "{text}");
    let written: Vec<&str> =
        text.lines().filter(|line| line.contains("at the default level")).collect();
    assert_eq!(written.len(), 1, "{text}");
    let under_default = written.first().is_some_and(|line| line.contains("[WARN] [default] "));
    assert!(under_default, "{written:?}");
    assert_eq!(health().rejected_lines, 0, "a fallback line is not rejected");
    Ok(())
}

#[test]
fn enabled_past_256_modules_follows_the_default_level() -> TestResult {
    in_own_process(
        "enabled_past_256_modules_follows_the_default_level",
        enabled_past_256_modules_follows_the_default_level_body,
    )
}

fn enabled_past_256_modules_follows_the_default_level_body() -> TestResult {
    hypha_log::install();
    let file = configure_default_level("overflow_facade", "info")?;
    fill_the_module_table()?;
    log::info!(target: "hypha_late_crate", "through the log facade");
    log::debug!(target: "hypha_late_crate", "below the default level");
    let text = std::fs::read_to_string(file)?;
    let written = text.lines().filter(|line| line.contains("through the log facade")).count();
    assert_eq!(written, 1, "{text}");
    assert!(!text.contains("below the default level"), "{text}");
    Ok(())
}
