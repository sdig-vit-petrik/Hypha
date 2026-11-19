//! Records of the `log` crate reach the engine, and the module level decides.
#![allow(clippy::expect_used)]

mod support;

use hypha_log::{commit, install, prepare};
use std::path::Path;
use support::{TestResult, in_own_process, log_table, parameter_file, temp_dir};

/// Commit a file sink at trace, with `extra` appended to the `[log]` table, and install
/// the facade.
fn commit_file_sink(dir: &Path, extra: &str) -> TestResult {
    let source = format!(
        "[log]\nlevel = \"debug\"\nconsole = false\n\
         [log.sink.a]\npath = \"out.log\"\nlevel = \"trace\"\n{extra}"
    );
    let prepared = prepare(&log_table(&source)?, &parameter_file(dir))
        .map_err(|error| format!("{error:?}"))?;
    commit(prepared).expect("commit");
    install();
    Ok(())
}

#[test]
fn a_log_record_reaches_a_sink_only_at_the_module_level() -> TestResult {
    in_own_process(
        "a_log_record_reaches_a_sink_only_at_the_module_level",
        a_log_record_reaches_a_sink_only_at_the_module_level_body,
    )
}

fn a_log_record_reaches_a_sink_only_at_the_module_level_body() -> TestResult {
    let dir = temp_dir("log_facade")?;
    commit_file_sink(&dir, "")?;
    install();

    log::warn!(target: "hypha/test_facade", "facade warning");
    log::trace!(target: "hypha/test_facade", "facade trace below the module level");
    log::warn!(target: "iceoryx2::service", "origin with a module path");

    let content = std::fs::read_to_string(dir.join("out.log"))?;
    assert!(
        content.contains("[WARN] [hypha/test_facade] facade warning\n"),
        "warning missing from sink: {content}"
    );
    assert!(!content.contains("facade trace"), "trace passed the module level: {content}");
    assert!(
        content.contains("[WARN] [iceoryx2/service] origin with a module path\n"),
        "module path was not mapped to an engine name: {content}"
    );
    Ok(())
}

#[test]
fn a_third_party_debug_record_is_dropped_by_default() -> TestResult {
    in_own_process(
        "a_third_party_debug_record_is_dropped_by_default",
        a_third_party_debug_record_is_dropped_by_default_body,
    )
}

fn a_third_party_debug_record_is_dropped_by_default_body() -> TestResult {
    let dir = temp_dir("log_facade_third_party_debug")?;
    commit_file_sink(&dir, "")?;

    log::debug!(target: "hypha/control", "control line");
    log::debug!(target: "tungstenite::protocol", "third party debug");

    let content = std::fs::read_to_string(dir.join("out.log"))?;
    assert!(content.contains("control line"), "control line missing: {content}");
    assert!(!content.contains("third party debug"), "third-party debug passed: {content}");
    Ok(())
}

#[test]
fn a_third_party_warning_is_kept() -> TestResult {
    in_own_process("a_third_party_warning_is_kept", a_third_party_warning_is_kept_body)
}

fn a_third_party_warning_is_kept_body() -> TestResult {
    let dir = temp_dir("log_facade_third_party_warning")?;
    commit_file_sink(&dir, "")?;

    log::warn!(target: "tungstenite::protocol", "third party warning");

    let content = std::fs::read_to_string(dir.join("out.log"))?;
    assert!(
        content.contains("[WARN] [tungstenite/protocol] third party warning\n"),
        "third-party warning missing: {content}"
    );
    Ok(())
}

#[test]
fn a_configured_third_party_module_keeps_its_level() -> TestResult {
    in_own_process(
        "a_configured_third_party_module_keeps_its_level",
        a_configured_third_party_module_keeps_its_level_body,
    )
}

fn a_configured_third_party_module_keeps_its_level_body() -> TestResult {
    let dir = temp_dir("log_facade_configured_third_party")?;
    commit_file_sink(&dir, "[log.module.\"tungstenite/protocol\"]\nlevel = \"debug\"\n")?;

    log::debug!(target: "tungstenite::protocol", "configured third party debug");

    let content = std::fs::read_to_string(dir.join("out.log"))?;
    assert!(
        content.contains("[DEBUG] [tungstenite/protocol] configured third party debug\n"),
        "configured third-party debug missing: {content}"
    );
    Ok(())
}

#[test]
fn a_hypha_debug_record_is_kept() -> TestResult {
    in_own_process("a_hypha_debug_record_is_kept", a_hypha_debug_record_is_kept_body)
}

fn a_hypha_debug_record_is_kept_body() -> TestResult {
    let dir = temp_dir("log_facade_hypha_debug")?;
    commit_file_sink(&dir, "")?;

    log::debug!(target: "hypha/transport", "hypha debug");

    let content = std::fs::read_to_string(dir.join("out.log"))?;
    assert!(
        content.contains("[DEBUG] [hypha/transport] hypha debug\n"),
        "hypha debug missing: {content}"
    );
    Ok(())
}
