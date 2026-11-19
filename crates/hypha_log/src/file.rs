//! The file side of a `[log.sink.<name>]` target: an append-only file that
//! rolls by size, and an exclusive lock on `<path>.lock`.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::LogError;
use crate::engine::lock;
use crate::level::Level;

fn invalid(path: &Path, error: impl std::fmt::Display) -> LogError {
    LogError::InvalidArgument(format!("{}: {error}", path.display()))
}

/// Lock `<path>.lock`. The lock file stays in place across rotations.
fn lock_file(path: &Path) -> Result<File, LogError> {
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(numbered(path, "lock"))
        .map_err(|error| invalid(path, format!("cannot open the lock file: {error}")))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(LogError::InvalidState(format!(
            "{}: another holder has locked the file",
            path.display()
        ))),
        Err(TryLockError::Error(error)) => {
            Err(invalid(path, format!("cannot lock the file: {error}")))
        }
    }
}

/// `<path>.<suffix>`.
fn numbered(path: &Path, suffix: impl std::fmt::Display) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{suffix}"));
    PathBuf::from(name)
}

/// Open `path` for append and read its current size.
fn open_append(path: &Path) -> io::Result<Writer> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let len = file.metadata()?.len();
    Ok(Writer { file, len })
}

/// Rename a file, treating a missing source as already moved.
fn shift(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// The time a sink waits after a failed roll before it tries again.
pub(crate) const ROLL_RETRY: Duration = Duration::from_secs(10);

/// The open file and the bytes it holds.
#[derive(Debug)]
struct Writer {
    file: File,
    len: u64,
}

/// The state behind a sink's mutex.
#[derive(Debug, Default)]
struct SinkState {
    writer: Option<Writer>,
    retry: RollRetry,
}

/// When a sink whose roll failed may try again. `None` while no failure is pending.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct RollRetry {
    next_attempt: Option<Instant>,
}

impl RollRetry {
    /// True when a roll may be tried at `now`.
    fn due(self, now: Instant) -> bool {
        self.next_attempt.is_none_or(|next| now >= next)
    }

    /// Record a failed attempt at `now`. True for the first failure of a failure period.
    fn failed(&mut self, now: Instant) -> bool {
        let first_of_period = self.next_attempt.is_none();
        self.next_attempt = Some(now.checked_add(ROLL_RETRY).unwrap_or(now));
        first_of_period
    }

    /// A successful roll ends the failure period.
    fn succeeded(&mut self) {
        self.next_attempt = None;
    }
}

/// What one line did to a sink.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WriteOutcome {
    /// The line is in the file.
    Written,
    /// The line is in the file, and the roll it made due failed.
    /// `first_of_period` is true for the first failure after a success or at start.
    WrittenRollFailed { first_of_period: bool, error: String },
    /// The line could not be written.
    Failed,
}

/// One file sink: an append-only file that rolls to `<path>.1` past `rotation_bytes`,
/// keeping `rotation_count` old files, and its lock.
#[derive(Debug)]
pub(crate) struct LogFile {
    level: Level,
    path: PathBuf,
    rotation_bytes: u64,
    rotation_count: u32,
    state: Mutex<SinkState>,
    _lock: File,
}

/// Create the parent directory, lock `<path>.lock`, and open `path` for append.
///
/// # Errors
///
/// `InvalidArgument` when the directory or a file cannot be made, and
/// `InvalidState` when another holder has locked the file.
pub(crate) fn open(
    path: &Path,
    rotation_bytes: u64,
    rotation_count: u32,
    level: Level,
) -> Result<LogFile, LogError> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|error| invalid(parent, format!("cannot create the directory: {error}")))?;
    }
    let lock = lock_file(path)?;
    let writer = open_append(path)
        .map_err(|error| invalid(path, format!("cannot open for append: {error}")))?;
    Ok(LogFile {
        level,
        path: path.to_path_buf(),
        rotation_bytes,
        rotation_count,
        state: Mutex::new(SinkState { writer: Some(writer), retry: RollRetry::default() }),
        _lock: lock,
    })
}

impl LogFile {
    /// The lowest level this sink writes.
    pub(crate) fn level(&self) -> Level {
        self.level
    }

    /// The file this sink writes.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Append one rendered line and flush it. A line that takes the file past its limit
    /// rolls it, unless a roll failed less than [`ROLL_RETRY`] ago.
    pub(crate) fn write_line(&self, line: &[u8]) -> WriteOutcome {
        let mut state = lock(&self.state);
        self.append(&mut state, line, Instant::now())
    }

    fn append(&self, state: &mut SinkState, line: &[u8], now: Instant) -> WriteOutcome {
        if state.writer.is_none() {
            let Ok(writer) = open_append(&self.path) else {
                return WriteOutcome::Failed;
            };
            state.writer = Some(writer);
        }
        let Some(writer) = state.writer.as_mut() else {
            return WriteOutcome::Failed;
        };
        let written = writer.file.write_all(line).and_then(|()| writer.file.flush());
        if written.is_err() {
            return WriteOutcome::Failed;
        }
        writer.len = writer.len.saturating_add(line.len() as u64);
        let roll_due = writer.len > self.rotation_bytes && state.retry.due(now);
        if !roll_due {
            return WriteOutcome::Written;
        }
        let closed = state.writer.take();
        match self.roll() {
            Ok(()) => {
                state.retry.succeeded();
                WriteOutcome::Written
            }
            Err(error) => {
                // The file did not move: keep writing to it until the next attempt.
                state.writer = closed;
                let first_of_period = state.retry.failed(now);
                WriteOutcome::WrittenRollFailed { first_of_period, error: error.to_string() }
            }
        }
    }

    /// `<path>.<i>` to `<path>.<i+1>` from the highest `i` down, then `<path>`
    /// to `<path>.1`. `<path>.<rotation_count>` is replaced.
    fn roll(&self) -> io::Result<()> {
        for index in (1..self.rotation_count).rev() {
            shift(&numbered(&self.path, index), &numbered(&self.path, index.saturating_add(1)))?;
        }
        std::fs::rename(&self.path, numbered(&self.path, 1))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "an instant past the test start is a setup failure")]
mod tests {
    use super::{ROLL_RETRY, RollRetry};
    use std::time::{Duration, Instant};

    fn after(start: Instant, span: Duration) -> Instant {
        start.checked_add(span).expect("an instant after the start")
    }

    #[test]
    fn a_failed_roll_is_retried_at_most_every_ten_seconds() {
        let start = Instant::now();
        let mut retry = RollRetry::default();
        assert!(retry.due(start), "no failure pending: a roll may run");
        assert!(retry.failed(start), "the first failure opens a period");
        assert!(!retry.due(start));
        let almost = after(start, Duration::from_millis(9_900));
        assert!(!retry.due(almost), "9.9 s after the failure");
        let ten = after(start, ROLL_RETRY);
        assert!(retry.due(ten), "10 s after the failure");
        assert!(!retry.failed(ten), "a later failure of the same period");
        assert!(!retry.due(after(ten, Duration::from_secs(9))));
    }

    #[test]
    fn a_successful_roll_starts_a_new_failure_period() {
        let start = Instant::now();
        let mut retry = RollRetry::default();
        assert!(retry.failed(start));
        retry.succeeded();
        assert!(retry.due(start), "the gate is cleared");
        assert!(retry.failed(start), "the next failure is the first of a new period");
    }
}
