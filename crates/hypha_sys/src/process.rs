//! A child process that ends with its parent. Linux only: other systems have no
//! parent-death signal.

use std::os::raw::{c_int, c_ulong};
use std::os::unix::process::CommandExt as _;
use std::process::Command;

/// The signal numbers Linux defines, real-time signals aside: 1 (SIGHUP) to 31 (SIGSYS).
const SIGNALS: std::ops::RangeInclusive<c_int> = 1..=31;

/// Make the child that `command` spawns get `signal` when this process dies; the spawn
/// fails with `ESRCH` when this process died first.
///
/// # Errors
/// `InvalidInput` for a number that is not a signal.
pub fn set_parent_death_signal(command: &mut Command, signal: c_int) -> std::io::Result<()> {
    if !SIGNALS.contains(&signal) {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    }
    let parent = libc::pid_t::try_from(std::process::id())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::ESRCH))?;
    let hook = move || death_signal_hook(parent, signal);
    // SAFETY: the hook makes only async-signal-safe calls and allocates nothing.
    unsafe { command.pre_exec(hook) };
    Ok(())
}

/// The pre-exec step: ask for `signal` at the parent's death, then check the parent.
fn death_signal_hook(parent: libc::pid_t, signal: c_int) -> std::io::Result<()> {
    let number = c_ulong::from(signal.unsigned_abs());
    // SAFETY: `PR_SET_PDEATHSIG` takes the signal number as its one argument and sets
    // an attribute of this process alone.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, number) } != 0 {
        // No allocation in the forked child: the raw errno.
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `getppid` has no precondition and cannot fail.
    parent_is(parent, unsafe { libc::getppid() })
}

/// `Ok` when `actual` is `parent`. An error makes the spawn fail before exec.
fn parent_is(parent: libc::pid_t, actual: libc::pid_t) -> std::io::Result<()> {
    if actual == parent {
        return Ok(());
    }
    // No allocation in the forked child: a raw errno, not a formatted message.
    Err(std::io::Error::from_raw_os_error(libc::ESRCH))
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "a test asserts the refusal")]
mod tests {
    use super::{parent_is, set_parent_death_signal};

    #[test]
    fn a_node_started_after_its_supervisor_died_does_not_run() {
        let supervisor = 4_000_000;
        assert!(parent_is(supervisor, supervisor).is_ok());
        let error = parent_is(supervisor, 1).expect_err("the parent changed");
        assert_eq!(error.raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn a_number_that_is_no_signal_is_refused() {
        for number in [0, -1, 32, 64] {
            let mut command = std::process::Command::new("true");
            let error = set_parent_death_signal(&mut command, number).expect_err("no signal");
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "signal {number}");
        }
        let mut command = std::process::Command::new("true");
        assert!(set_parent_death_signal(&mut command, libc::SIGTERM).is_ok());
    }
}
