//! The console never blocks the thread that logs.
//! Linux only: off Linux a pipe on fd 2 is written as it is (`console.rs`).
#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::arithmetic_side_effects)]

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use hypha_log::{Level, commit, health, prepare, register_module, write};
use support::{TestResult, log_table, parameter_file, temp_dir};

const CHILD_VARIABLE: &str = "HYPHA_LOG_CONSOLE_CHILD";
const TEST_NAME: &str = "a_full_stderr_pipe_never_blocks_and_the_dropped_count_is_written_once";
const FIRST_LINES: usize = 2_000;
const SECOND_LINES: usize = 10;
const FINAL_LINE: &str = "the line after the reader came back";

fn line(number: usize) -> Vec<u8> {
    format!("console line {number:06} {}", "x".repeat(80)).into_bytes()
}

/// The child: log until the pipe is full, report, log more, then log once more
/// after the parent has started reading.
fn child() -> TestResult {
    let dir = temp_dir("console_child")?;
    let source = "[log]\nconsole = true\n[log.sink.main]\npath = \"all.log\"\nlevel = \"info\"\n";
    let prepared = prepare(&log_table(source)?, &parameter_file(&dir))
        .map_err(|error| format!("{error:?}"))?;
    commit(prepared).expect("commit");
    let module = register_module("console")?;
    let mut stdout = std::io::stdout();
    let mut stdin = BufReader::new(std::io::stdin());
    for number in 1..=FIRST_LINES {
        write(module, Level::Info, &line(number));
    }
    writeln!(stdout, "dropped {}", health().write_failures)?;
    for number in 1..=SECOND_LINES {
        write(module, Level::Info, &line(FIRST_LINES + number));
    }
    // The notice failed with every line of the second phase, so the count grew.
    writeln!(stdout, "dropped {}", health().write_failures)?;
    stdout.flush()?;
    let mut go = String::new();
    stdin.read_line(&mut go)?;
    write(module, Level::Info, FINAL_LINE.as_bytes());
    let flags = rustix::fs::fcntl_getfl(std::io::stderr())?;
    let shared_is_blocking = !flags.contains(rustix::fs::OFlags::NONBLOCK);
    writeln!(stdout, "fd2 blocking {shared_is_blocking}")?;
    let file = std::fs::read_to_string(dir.join("all.log"))?;
    let in_file = file.lines().count();
    writeln!(stdout, "file lines {in_file}")?;
    stdout.flush()?;
    Ok(())
}

/// What follows `prefix` on the next line of the child's stdout that holds it.
fn next_value(
    reader: &mut impl BufRead,
    prefix: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    loop {
        let mut text = String::new();
        let read = reader.read_line(&mut text)?;
        if read == 0 {
            return Err(format!("the child ended before `{prefix}`").into());
        }
        // The harness writes `test <name> ... ` without a newline before the first report.
        if let Some(start) = text.find(prefix) {
            let value = text.get(start..).and_then(|rest| rest.strip_prefix(prefix));
            return Ok(value.unwrap_or_default().trim_end().to_owned());
        }
    }
}

fn wait_with_deadline(child: &mut Child, deadline: Duration) -> TestResult {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            assert!(status.success(), "the child failed: {status}");
            return Ok(());
        }
        let too_long = started.elapsed() > deadline;
        if too_long {
            child.kill()?;
            return Err("the child blocked on its full standard error".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[cfg_attr(miri, ignore = "spawns a child process on a pipe, which Miri cannot")]
fn a_full_stderr_pipe_never_blocks_and_the_dropped_count_is_written_once() -> TestResult {
    if std::env::var_os(CHILD_VARIABLE).is_some() {
        return child();
    }
    let mut spawned = Command::new(std::env::current_exe()?)
        .args(["--exact", TEST_NAME, "--nocapture", "--test-threads", "1"])
        .env(CHILD_VARIABLE, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = BufReader::new(spawned.stdout.take().expect("child stdout"));
    let mut stdin = spawned.stdin.take().expect("child stdin");
    let mut stderr = spawned.stderr.take().expect("child stderr");

    // Nobody reads standard error yet: the child must still get through 2,000 lines.
    let started = Instant::now();
    let first: u64 = next_value(&mut stdout, "dropped ")?.parse()?;
    assert!(started.elapsed() < Duration::from_secs(10), "the child blocked");
    assert!(first > 0, "a 64 KiB pipe cannot hold {FIRST_LINES} lines of 100 bytes");
    let second: u64 = next_value(&mut stdout, "dropped ")?.parse()?;
    assert_eq!(second, first + SECOND_LINES as u64, "each line of a full pipe is dropped");

    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let read = stderr.read_to_string(&mut text);
        read.map(|_| text)
    });
    // Let the reader empty the pipe before the child writes its last line.
    std::thread::sleep(Duration::from_millis(200));
    writeln!(stdin, "go")?;
    let blocking = next_value(&mut stdout, "fd2 blocking ")?;
    assert_eq!(blocking, "true", "the shared descriptor keeps its flags");
    let in_file: usize = next_value(&mut stdout, "file lines ")?.parse()?;
    assert_eq!(in_file, FIRST_LINES + SECOND_LINES + 1, "the file sink has every line");
    wait_with_deadline(&mut spawned, Duration::from_secs(10))?;

    let text = reader.join().map_err(|_| "the reader panicked")??;
    let notice = format!("{second} log lines dropped: standard error was not read");
    let notices: Vec<&str> =
        text.lines().filter(|line| line.contains("log lines dropped")).collect();
    assert_eq!(notices.len(), 1, "one notice: {notices:?}");
    let last_two: Vec<&str> = text.lines().rev().take(2).collect();
    let notice_then_line = last_two.get(1).is_some_and(|line| line.ends_with(&notice))
        && last_two.first().is_some_and(|line| line.ends_with(FINAL_LINE));
    assert!(notice_then_line, "the notice comes right before the next line: {last_two:?}");
    let whole_lines = text.lines().all(|line| line.starts_with('['));
    assert!(whole_lines, "no line was cut");
    Ok(())
}
