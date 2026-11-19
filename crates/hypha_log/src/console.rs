//! The console target: standard error, written without blocking the thread that logs.
//!
//! On Linux a pipe or terminal is reopened through `/proc/self/fd/2` with `O_NONBLOCK`,
//! so the shared descriptor's flags are not changed; a socket gets `send(MSG_DONTWAIT)`.
//! Each line is one write call; a line not written whole is dropped and counted, and
//! the next whole line is preceded by one notice with the count.

use std::fs::File;
use std::io::{self, Write};
use std::sync::Mutex;

use crate::engine::lock;
use crate::level::Level;
use crate::render::render_line;

/// What became of one console line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsoleOutcome {
    /// The line, and a pending notice before it, were written whole.
    Written,
    /// The line was dropped and counted.
    Dropped,
}

/// How standard error is written.
#[derive(Debug)]
enum Target {
    /// A non-blocking file description of the same pipe or terminal.
    Reopened(File),
    /// A socket, sent to with `MSG_DONTWAIT`.
    Socket,
    /// A regular file, or a pipe or terminal off Linux: written as it is.
    Plain,
    /// A pipe or terminal that could not be reopened: every line is dropped and counted.
    Unavailable,
}

/// The file that fd 2 names: its device and inode.
type Identity = (rustix::fs::Dev, u64);

/// The state behind the console mutex.
#[derive(Debug)]
struct ConsoleState {
    /// Lines dropped since the last line that was written whole.
    pending: u64,
    /// How fd 2 is written, and the file it named when that was decided.
    target: Option<(Identity, Target)>,
}

/// The console target of the engine.
#[derive(Debug)]
pub(crate) struct Console {
    state: Mutex<ConsoleState>,
}

impl Console {
    pub(crate) const fn new() -> Self {
        Self { state: Mutex::new(ConsoleState { pending: 0, target: None }) }
    }

    /// True when standard error cannot be written without blocking.
    pub(crate) fn unavailable(&self) -> bool {
        let state = lock(&self.state);
        matches!(state.target, Some((_, Target::Unavailable)))
    }

    /// Write the rendered `line` with one write call, a pending count first as a notice.
    /// When the notice is not written whole, the line is dropped too.
    pub(crate) fn write_line(&self, line: &[u8]) -> ConsoleOutcome {
        let mut guard = lock(&self.state);
        let ConsoleState { pending, target: slot } = &mut *guard;
        let target = current_target(slot);
        if *pending > 0 {
            let text = format!("{} log lines dropped: standard error was not read", *pending);
            let notice = render_line(Level::Warn, "hypha", text.as_bytes());
            let notice_written = write_whole(target, &notice.bytes);
            if !notice_written {
                *pending = pending.wrapping_add(1);
                return ConsoleOutcome::Dropped;
            }
            *pending = 0;
        }
        if write_whole(target, line) {
            return ConsoleOutcome::Written;
        }
        *pending = pending.wrapping_add(1);
        ConsoleOutcome::Dropped
    }
}

/// The target for the file fd 2 names now; decided again when that file changed.
fn current_target(slot: &mut Option<(Identity, Target)>) -> &Target {
    let identity = rustix::fs::fstat(io::stderr()).map(|status| (status.st_dev, status.st_ino));
    let unchanged = matches!((&*slot, &identity), (Some((held, _)), Ok(now)) if held == now);
    if !unchanged {
        let target = open_target();
        let identity = identity.unwrap_or_default();
        *slot = Some((identity, target));
    }
    match slot {
        Some((_, target)) => target,
        None => &Target::Unavailable,
    }
}

/// One write call. True when every byte of `bytes` was written.
fn write_whole(target: &Target, bytes: &[u8]) -> bool {
    let written = match target {
        Target::Reopened(file) => {
            let mut writer: &File = file;
            writer.write(bytes)
        }
        Target::Socket => send_without_waiting(bytes),
        Target::Plain => io::stderr().lock().write(bytes),
        Target::Unavailable => return false,
    };
    written.is_ok_and(|count| count == bytes.len())
}

/// Decide how fd 2, as it is now, is written.
fn open_target() -> Target {
    let Ok(status) = rustix::fs::fstat(io::stderr()) else {
        return Target::Plain;
    };
    let kind = rustix::fs::FileType::from_raw_mode(status.st_mode);
    match kind {
        rustix::fs::FileType::Socket => Target::Socket,
        rustix::fs::FileType::Fifo | rustix::fs::FileType::CharacterDevice => reopen(),
        _ => Target::Plain,
    }
}

/// A non-blocking file description of the pipe or terminal behind fd 2.
#[cfg(target_os = "linux")]
fn reopen() -> Target {
    let flags = rustix::fs::OFlags::WRONLY
        | rustix::fs::OFlags::APPEND
        | rustix::fs::OFlags::NONBLOCK
        | rustix::fs::OFlags::CLOEXEC;
    match rustix::fs::open("/proc/self/fd/2", flags, rustix::fs::Mode::empty()) {
        Ok(owned) => Target::Reopened(File::from(owned)),
        Err(_) => Target::Unavailable,
    }
}

/// Off Linux, `/dev/fd/2` would share the descriptor's flags: written as it is.
#[cfg(not(target_os = "linux"))]
fn reopen() -> Target {
    Target::Plain
}

#[cfg(target_os = "linux")]
fn send_without_waiting(bytes: &[u8]) -> io::Result<usize> {
    let flags = rustix::net::SendFlags::DONTWAIT | rustix::net::SendFlags::NOSIGNAL;
    rustix::net::send(io::stderr(), bytes, flags).map_err(io::Error::from)
}

#[cfg(not(target_os = "linux"))]
fn send_without_waiting(bytes: &[u8]) -> io::Result<usize> {
    rustix::net::send(io::stderr(), bytes, rustix::net::SendFlags::DONTWAIT)
        .map_err(io::Error::from)
}
