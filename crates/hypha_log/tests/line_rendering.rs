//! The text of a rendered line.
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

mod support;

use hypha_log::{Level, commit, prepare, register_module, write};
use hypha_time::Time;
use hypha_time::enable_sim_time;
use std::path::PathBuf;
use support::{TestResult, in_own_process, log_table, parameter_file, temp_dir};

/// Configure one trace-level file sink and return its path.
fn configure(tag: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let dir = temp_dir(tag)?;
    let source = "[log]\nlevel = \"trace\"\nconsole = false\n\
        [log.sink.a]\npath = \"out.log\"\nlevel = \"trace\"\n";
    let prepared = prepare(&log_table(source)?, &parameter_file(&dir))
        .map_err(|error| format!("{error:?}"))?;
    commit(prepared).expect("commit");
    Ok(dir.join("out.log"))
}

#[test]
fn a_line_has_wall_time_level_module_and_text() -> TestResult {
    in_own_process(
        "a_line_has_wall_time_level_module_and_text",
        a_line_has_wall_time_level_module_and_text_body,
    )
}

fn a_line_has_wall_time_level_module_and_text_body() -> TestResult {
    let path = configure("render_plain")?;
    let module = register_module("scan_cycle")?;
    write(module, Level::Warn, b"speed is stale");
    let content = std::fs::read_to_string(path)?;
    let line = content.strip_suffix('\n').ok_or("the line has no newline")?;
    let (stamp, rest) = line.split_at_checked(26).ok_or("line too short")?;
    assert_eq!(rest, " [WARN] [scan_cycle] speed is stale");
    let shape = stamp.as_bytes();
    assert_eq!(shape.first(), Some(&b'['));
    assert_eq!(shape.get(5), Some(&b'-'));
    assert_eq!(shape.get(11), Some(&b'T'));
    assert_eq!(shape.get(24), Some(&b'Z'));
    assert_eq!(shape.get(25), Some(&b']'));
    assert!(stamp.starts_with("[20"), "the wall clock gives the current epoch year: {stamp}");
    Ok(())
}

#[test]
fn every_level_has_its_name() -> TestResult {
    in_own_process("every_level_has_its_name", every_level_has_its_name_body)
}

fn every_level_has_its_name_body() -> TestResult {
    let path = configure("render_levels")?;
    let module = register_module("m")?;
    let levels = [
        (Level::Trace, "TRACE"),
        (Level::Debug, "DEBUG"),
        (Level::Info, "INFO"),
        (Level::Warn, "WARN"),
        (Level::Error, "ERROR"),
        (Level::Fatal, "FATAL"),
    ];
    for (level, _) in levels {
        write(module, level, b"x");
    }
    let content = std::fs::read_to_string(path)?;
    for ((_, name), line) in levels.iter().zip(content.lines()) {
        assert!(line.contains(&format!("[{name}] [m] x")), "{line}");
    }
    Ok(())
}

#[test]
fn simulated_time_adds_a_second_field() -> TestResult {
    in_own_process("simulated_time_adds_a_second_field", simulated_time_adds_a_second_field_body)
}

fn simulated_time_adds_a_second_field_body() -> TestResult {
    let path = configure("render_sim")?;
    assert!(enable_sim_time(Time::from_nanos(412_250_000_000)));
    let module = register_module("sim")?;
    write(module, Level::Info, b"tick");
    let content = std::fs::read_to_string(path)?;
    assert!(content.contains("Z] [sim 412.250000] [INFO] [sim] tick\n"), "{content}");
    Ok(())
}

#[test]
fn a_line_written_in_a_step_shows_the_steps_time() -> TestResult {
    in_own_process(
        "a_line_written_in_a_step_shows_the_steps_time",
        a_line_written_in_a_step_shows_the_steps_time_body,
    )
}

fn a_line_written_in_a_step_shows_the_steps_time_body() -> TestResult {
    let path = configure("render_step")?;
    // The clock is at the chunk end; a catch-up step runs at an earlier deadline.
    assert!(enable_sim_time(Time::from_nanos(412_250_000_000)));
    let module = register_module("step")?;
    {
        let _step = hypha_time::StepTimeGuard::hold(412_100_000_000);
        write(module, Level::Info, b"in the step");
    }
    write(module, Level::Info, b"after the step");
    let content = std::fs::read_to_string(path)?;
    assert!(content.contains("[sim 412.100000] [INFO] [step] in the step\n"), "{content}");
    assert!(content.contains("[sim 412.250000] [INFO] [step] after the step\n"), "{content}");
    Ok(())
}

#[test]
fn multiline_entries_preserve_line_breaks() -> TestResult {
    in_own_process(
        "multiline_entries_preserve_line_breaks",
        multiline_entries_preserve_line_breaks_body,
    )
}

fn multiline_entries_preserve_line_breaks_body() -> TestResult {
    let path = configure("render_multiline")?;
    let module = register_module("esc")?;
    write(module, Level::Info, b"a\nb\r\nc");
    write(module, Level::Info, b"name=cycle\nperiod_nanos=1000000\n");
    let content = std::fs::read_to_string(path)?;
    assert_eq!(content.lines().count(), 5);
    assert_eq!(content.matches("[INFO] [esc]").count(), 2);
    assert!(content.contains("[INFO] [esc] a\nb\r\nc\n"), "{content:?}");
    assert!(content.ends_with("name=cycle\nperiod_nanos=1000000\n"), "{content:?}");
    assert_eq!(hypha_log::health().lines_written, 2);
    Ok(())
}

#[test]
fn a_message_over_16_kib_is_cut_and_marked() -> TestResult {
    in_own_process(
        "a_message_over_16_kib_is_cut_and_marked",
        a_message_over_16_kib_is_cut_and_marked_body,
    )
}

fn a_message_over_16_kib_is_cut_and_marked_body() -> TestResult {
    let path = configure("render_cut")?;
    let module = register_module("cut")?;
    write(module, Level::Info, &vec![b'x'; 20_000]);
    write(module, Level::Info, &vec![b'y'; 16_384]);
    let content = std::fs::read_to_string(path)?;
    let mut lines = content.lines();
    let cut = lines.next().ok_or("no first line")?;
    let exact = lines.next().ok_or("no second line")?;
    assert!(cut.ends_with(&format!("{} [truncated 3616 bytes]", "x".repeat(16_384))));
    assert!(exact.ends_with(&"y".repeat(16_384)));
    assert!(!exact.contains("truncated"));
    Ok(())
}

#[test]
fn bytes_that_are_not_utf8_are_written_as_received() -> TestResult {
    in_own_process(
        "bytes_that_are_not_utf8_are_written_as_received",
        bytes_that_are_not_utf8_are_written_as_received_body,
    )
}

fn bytes_that_are_not_utf8_are_written_as_received_body() -> TestResult {
    let path = configure("render_bytes")?;
    let module = register_module("raw")?;
    write(module, Level::Info, &[0xff, 0xfe, b'!']);
    let content = std::fs::read(path)?;
    assert!(content.ends_with(&[0xff, 0xfe, b'!', b'\n']), "the bytes are written as received");
    Ok(())
}

#[test]
fn a_file_never_contains_a_colour_code() -> TestResult {
    in_own_process("a_file_never_contains_a_colour_code", a_file_never_contains_a_colour_code_body)
}

fn a_file_never_contains_a_colour_code_body() -> TestResult {
    let path = configure("render_colour")?;
    let module = register_module("col")?;
    write(module, Level::Fatal, b"red if it were a terminal");
    let content = std::fs::read(path)?;
    assert!(!content.contains(&0x1b));
    Ok(())
}
