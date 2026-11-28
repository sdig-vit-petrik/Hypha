//! Whether a process lives.

/// True while the process `pid` lives, also when it is another user's. Process ids 0
/// and 1 name no ordinary process (0 would be this process group, 1 is init), so they
/// do not live here.
#[must_use]
pub fn process_lives(pid: u32) -> bool {
    let Ok(raw) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if raw <= 1 {
        return false;
    }
    // SAFETY: signal 0 sends nothing; `kill` only reports whether the process exists.
    let exists = unsafe { libc::kill(raw, 0) } == 0;
    // A process of another user refuses the signal, and lives.
    exists || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::process_lives;

    #[test]
    fn this_process_lives_and_no_ordinary_process_has_id_0_or_1() {
        assert!(process_lives(std::process::id()));
        assert!(!process_lives(0));
        assert!(!process_lives(1));
        assert!(!process_lives(u32::MAX));
    }
}
