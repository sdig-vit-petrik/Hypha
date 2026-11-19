//! A child process that ends with its parent. Linux only: other systems have no
//! parent-death signal.

use std::os::raw::c_int;
use std::os::unix::process::CommandExt as _;
use std::process::Command;

use nix::sys::signal::Signal;
use nix::unistd::Pid;

/// Make the child that `command` spawns get `signal` when this process dies; the spawn
/// fails with `ESRCH` when this process died first.
///
/// # Errors
/// `InvalidInput` for a number that is not a signal.
pub fn set_parent_death_signal(command: &mut Command, signal: c_int) -> std::io::Result<()> {
    let signal = Signal::try_from(signal).map_err(std::io::Error::from)?;
    let parent = nix::unistd::getpid();
    let hook = move || death_signal_hook(parent, signal);
    // SAFETY: the hook makes only async-signal-safe calls and allocates nothing.
    unsafe { command.pre_exec(hook) };
    Ok(())
}

/// The pre-exec step: ask for `signal` at the parent's death, then check the parent.
fn death_signal_hook(parent: Pid, signal: Signal) -> std::io::Result<()> {
    nix::sys::prctl::set_pdeathsig(signal).map_err(std::io::Error::from)?;
    parent_is(parent, nix::unistd::getppid())
}

/// `Ok` when `actual` is `parent`. An error makes the spawn fail before exec.
fn parent_is(parent: Pid, actual: Pid) -> std::io::Result<()> {
    if actual == parent {
        return Ok(());
    }
    // No allocation in the forked child: a raw errno, not a formatted message.
    Err(std::io::Error::from_raw_os_error(libc::ESRCH))
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "a test asserts the refusal")]
mod tests {
    use super::{Pid, parent_is};

    #[test]
    fn a_node_started_after_its_supervisor_died_does_not_run() {
        let supervisor = Pid::from_raw(4_000_000);
        assert!(parent_is(supervisor, supervisor).is_ok());
        let error = parent_is(supervisor, Pid::from_raw(1)).expect_err("the parent changed");
        assert_eq!(error.raw_os_error(), Some(libc::ESRCH));
    }
}
