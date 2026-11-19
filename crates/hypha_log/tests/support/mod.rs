//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::error::Error;
use std::path::{Path, PathBuf};

pub(crate) type TestResult = Result<(), Box<dyn Error>>;

/// A fresh empty directory for one test.
pub(crate) fn temp_dir(tag: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("hypha_log_{}_{tag}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The `log` table of a TOML document.
pub(crate) fn log_table(source: &str) -> Result<toml::Table, Box<dyn Error>> {
    let mut document: toml::Table = source.parse()?;
    match document.remove("log") {
        Some(toml::Value::Table(table)) => Ok(table),
        _ => Err("the document has no [log] table".into()),
    }
}

/// The path of a parameter file inside `dir`. The file need not exist.
pub(crate) fn parameter_file(dir: &Path) -> PathBuf {
    dir.join("node.toml")
}

/// What a test body may return: nothing, or a `TestResult`.
pub(crate) trait Outcome {
    fn into_test_result(self) -> TestResult;
}

impl Outcome for () {
    fn into_test_result(self) -> TestResult {
        Ok(())
    }
}

impl Outcome for TestResult {
    fn into_test_result(self) -> TestResult {
        self
    }
}

/// The variable that names the one test a child process runs.
const CHILD_VARIABLE: &str = "HYPHA_LOG_TEST_CHILD";

/// Run `body` in a child process of this test binary, unless this process is that
/// child: the logging configuration is one value per process. Under Miri the body runs
/// in the test's own process.
pub(crate) fn in_own_process<T: Outcome>(test: &str, body: fn() -> T) -> TestResult {
    let is_child = std::env::var(CHILD_VARIABLE).as_deref() == Ok(test);
    let has_own_process =
        cfg!(miri) && std::env::var("NEXTEST_EXECUTION_MODE").as_deref() == Ok("process-per-test");
    if is_child || has_own_process {
        return body().into_test_result();
    }
    let status = std::process::Command::new(std::env::current_exe()?)
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_VARIABLE, test)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{test} failed in its own process: {status}").into())
    }
}
