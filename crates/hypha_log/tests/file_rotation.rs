//! Size-based rotation of a file sink.
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

mod support;

use hypha_log::{Level, commit, health, prepare, register_module, write};
use std::path::{Path, PathBuf};
use support::{TestResult, in_own_process, log_table, parameter_file, temp_dir};

const LINE_BODY: usize = 16_000;
/// Lines of 16 039 bytes: the 66th takes a file past 1 MiB and is its last line.
const LINES_PER_FILE: usize = 66;

fn numbered_line(number: usize) -> Vec<u8> {
    let mut text = format!("n{number:06} ").into_bytes();
    text.resize(LINE_BODY, b'.');
    text
}

fn numbers_in(path: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    Ok(content
        .lines()
        .filter_map(|line| line.split_once("[m] n"))
        .map(|(_, rest)| rest.chars().take(6).collect())
        .collect())
}

fn configure(dir: &Path, count: u32) -> TestResult {
    let source = format!(
        "[log]\nconsole = false\n[log.sink.a]\npath = \"r.log\"\nrotation_mb = 1\nrotation_count = {count}\n"
    );
    let prepared = prepare(&log_table(&source)?, &parameter_file(dir))
        .map_err(|error| format!("{error:?}"))?;
    commit(prepared).expect("commit");
    Ok(())
}

fn sibling(path: &Path, suffix: u32) -> PathBuf {
    PathBuf::from(format!("{}.{suffix}", path.display()))
}

#[test]
fn rotation_keeps_the_newest_files_with_logrotate_names() -> TestResult {
    in_own_process(
        "rotation_keeps_the_newest_files_with_logrotate_names",
        rotation_keeps_the_newest_files_with_logrotate_names_body,
    )
}

fn rotation_keeps_the_newest_files_with_logrotate_names_body() -> TestResult {
    let dir = temp_dir("rotation_names")?;
    configure(&dir, 2)?;
    let module = register_module("m")?;
    let total = LINES_PER_FILE * 3 + 1;
    for number in 1..=total {
        write(module, Level::Info, &numbered_line(number));
    }
    let path = dir.join("r.log");
    assert_eq!(numbers_in(&path)?, vec![format!("{total:06}")]);
    let newest = numbers_in(&sibling(&path, 1))?;
    assert_eq!(newest.len(), LINES_PER_FILE);
    assert_eq!(newest.first(), Some(&format!("{:06}", LINES_PER_FILE * 2 + 1)));
    let older = numbers_in(&sibling(&path, 2))?;
    assert_eq!(older.len(), LINES_PER_FILE);
    assert_eq!(older.first(), Some(&format!("{:06}", LINES_PER_FILE + 1)));
    assert!(!sibling(&path, 3).exists(), "the oldest file is removed");
    assert_eq!(health().write_failures, 0);
    Ok(())
}

#[test]
fn rotation_keeps_at_most_rotation_count_numbered_files() -> TestResult {
    in_own_process(
        "rotation_keeps_at_most_rotation_count_numbered_files",
        rotation_keeps_at_most_rotation_count_numbered_files_body,
    )
}

fn rotation_keeps_at_most_rotation_count_numbered_files_body() -> TestResult {
    let dir = temp_dir("rotation_count")?;
    configure(&dir, 2)?;
    let module = register_module("m")?;
    for number in 1..=200 {
        write(module, Level::Info, &numbered_line(number));
    }
    let path = dir.join("r.log");
    assert!(path.exists());
    assert!(sibling(&path, 1).exists());
    assert!(sibling(&path, 2).exists());
    assert!(!sibling(&path, 3).exists(), "no file beyond rotation_count");
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore = "a scale test: a megabyte file takes Miri up to ten minutes")]
fn an_existing_file_is_appended_and_counted_toward_the_limit() -> TestResult {
    in_own_process(
        "an_existing_file_is_appended_and_counted_toward_the_limit",
        an_existing_file_is_appended_and_counted_toward_the_limit_body,
    )
}

fn an_existing_file_is_appended_and_counted_toward_the_limit_body() -> TestResult {
    let dir = temp_dir("rotation_append")?;
    let path = dir.join("r.log");
    std::fs::write(&path, vec![b'o'; 1_048_000])?;
    configure(&dir, 3)?;
    let module = register_module("m")?;
    write(module, Level::Info, &numbered_line(1));
    let rotated = std::fs::read(sibling(&path, 1))?;
    assert!(rotated.starts_with(&vec![b'o'; 1_048_000]), "the old content moved to .1 whole");
    assert!(numbers_in(&sibling(&path, 1))?.contains(&"000001".to_string()));
    assert!(
        !path.exists(),
        "the line that crossed the limit ended the file; the next line opens a new one"
    );
    write(module, Level::Info, &numbered_line(2));
    assert_eq!(numbers_in(&path)?, vec!["000002".to_string()]);
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore = "Miri outlasts the 10 s roll retry period it assumes")]
fn a_failed_rotation_is_a_write_failure_and_the_write_continues() -> TestResult {
    in_own_process(
        "a_failed_rotation_is_a_write_failure_and_the_write_continues",
        a_failed_rotation_is_a_write_failure_and_the_write_continues_body,
    )
}

fn a_failed_rotation_is_a_write_failure_and_the_write_continues_body() -> TestResult {
    let dir = temp_dir("rotation_failure")?;
    let path = dir.join("r.log");
    std::fs::create_dir(sibling(&path, 1))?;
    configure(&dir, 1)?;
    let module = register_module("m")?;
    let total = LINES_PER_FILE * 2;
    for number in 1..=total {
        write(module, Level::Info, &numbered_line(number));
    }
    assert_eq!(numbers_in(&path)?.len(), total, "the sink writes past the limit");
    // The line that crossed the limit tries the roll once; the next try is 10 s later.
    let counts = health();
    assert_eq!(counts.write_failures, 1);
    assert_eq!(counts.lines_written, total as u64, "every line was written");
    Ok(())
}

#[test]
#[cfg_attr(miri, ignore = "Miri outlasts the 10 s roll retry period it assumes")]
fn a_failed_roll_warns_once_per_failure_period() -> TestResult {
    in_own_process(
        "a_failed_roll_warns_once_per_failure_period",
        a_failed_roll_warns_once_per_failure_period_body,
    )
}

/// Sink `a` cannot roll; sink `b` gets the one warning, and `a` never does.
fn a_failed_roll_warns_once_per_failure_period_body() -> TestResult {
    let dir = temp_dir("rotation_warning")?;
    std::fs::create_dir(sibling(&dir.join("r.log"), 1))?;
    let source = "[log]\nconsole = false\n[log.sink.a]\npath = \"r.log\"\nrotation_mb = 1\nrotation_count = 1\n\
                  [log.sink.b]\npath = \"w.log\"\nlevel = \"warn\"\n";
    let prepared = prepare(&log_table(source)?, &parameter_file(&dir))
        .map_err(|error| format!("{error:?}"))?;
    commit(prepared).expect("commit");
    let module = register_module("m")?;
    for number in 1..=LINES_PER_FILE * 2 {
        write(module, Level::Info, &numbered_line(number));
    }
    let warnings = std::fs::read_to_string(dir.join("w.log"))?;
    let expected = format!("[WARN] [hypha] hypha_log: cannot roll {}", dir.join("r.log").display());
    let in_b = warnings.lines().filter(|line| line.contains(&expected)).count();
    assert_eq!(in_b, 1, "w.log: {warnings}");
    let ends_right = warnings.lines().any(|line| line.ends_with("; retrying every 10 s"));
    assert!(ends_right, "w.log: {warnings}");
    let failing = std::fs::read_to_string(dir.join("r.log"))?;
    assert!(!failing.contains("cannot roll"), "the failing sink holds no warning");
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn a_full_disk_is_counted_and_never_stops_the_caller() -> TestResult {
    in_own_process(
        "a_full_disk_is_counted_and_never_stops_the_caller",
        a_full_disk_is_counted_and_never_stops_the_caller_body,
    )
}

/// The sink is a symbolic link to `/dev/full`.
#[cfg(target_os = "linux")]
fn a_full_disk_is_counted_and_never_stops_the_caller_body() -> TestResult {
    let dir = temp_dir("rotation_full_disk")?;
    std::os::unix::fs::symlink("/dev/full", dir.join("full.log"))?;
    let source = "[log]\nconsole = false\n[log.sink.a]\npath = \"full.log\"\n";
    let prepared = prepare(&log_table(source)?, &parameter_file(&dir))
        .map_err(|error| format!("{error:?}"))?;
    commit(prepared).expect("commit");
    let module = register_module("full")?;
    for _ in 0..5 {
        write(module, Level::Error, b"disk is full");
    }
    let counts = health();
    assert_eq!(counts.write_failures, 5);
    assert_eq!(counts.lines_written, 0);
    Ok(())
}

#[test]
fn every_line_survives_rotation_whole_and_once() -> TestResult {
    in_own_process(
        "every_line_survives_rotation_whole_and_once",
        every_line_survives_rotation_whole_and_once_body,
    )
}

/// Three full files plus one line, with `rotation_count = 3`: no line is lost or split.
fn every_line_survives_rotation_whole_and_once_body() -> TestResult {
    let dir = temp_dir("rotation_whole")?;
    configure(&dir, 3)?;
    let module = register_module("m")?;
    let total = LINES_PER_FILE * 3 + 1;
    for number in 1..=total {
        write(module, Level::Info, &numbered_line(number));
    }
    let path = dir.join("r.log");
    let files = [sibling(&path, 3), sibling(&path, 2), sibling(&path, 1), path.clone()];
    let mut seen = Vec::new();
    for file in &files {
        let content = std::fs::read_to_string(file)?;
        for line in content.lines() {
            let (_, body) = line.split_once("[m] ").ok_or("a line without the prefix")?;
            assert_eq!(body.len(), LINE_BODY, "{}: a split or merged line", file.display());
            seen.push(body.get(1..7).ok_or("a short line")?.to_string());
        }
    }
    let expected: Vec<String> = (1..=total).map(|number| format!("{number:06}")).collect();
    assert_eq!(seen, expected, "every line once, in order, oldest file first");
    assert!(!sibling(&path, 4).exists(), "no file beyond rotation_count");
    assert_eq!(health().write_failures, 0);
    assert_eq!(health().lines_written, total as u64);
    Ok(())
}
