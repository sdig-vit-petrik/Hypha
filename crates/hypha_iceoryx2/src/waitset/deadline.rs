//! The wake-up at the dispatcher's timed deadline.
//!
//! The native wait takes a relative timeout. The Linux reactor hands it to `epoll_wait`
//! rounded up to a whole millisecond, so a wait for its timeout alone ends up to 1 ms after
//! the deadline. On Linux a `timerfd` on `CLOCK_MONOTONIC`, armed with the absolute deadline
//! and attached as a notification, ends the wait at the deadline itself. Elsewhere the
//! timeout is the wake-up: macOS's `select` takes microseconds, and the kernel's timer
//! coalescing defers its wake-up by up to half the wait.

use hypha_core::port::PortError;
use hypha_time::Time;
use iceoryx2::waitset::{WaitSet, WaitSetGuard};

use crate::transport::Ipc;

#[cfg(target_os = "linux")]
pub(super) use linux::DeadlineTimer;

#[cfg(target_os = "linux")]
mod linux {
    use std::os::fd::{AsRawFd, OwnedFd};

    use iceoryx2_bb_posix::file_descriptor::{FileDescriptor, FileDescriptorBased};
    use iceoryx2_bb_posix::file_descriptor_set::SynchronousMultiplexing;
    use rustix::time::{
        Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec, timerfd_create,
        timerfd_settime,
    };

    use super::{Ipc, PortError, Time, WaitSet, WaitSetGuard};

    const NANOS_PER_SECOND: i64 = 1_000_000_000;

    /// An expiry of zero disarms a `timerfd`.
    const DISARMED: Timespec = Timespec { tv_sec: 0, tv_nsec: 0 };

    /// A `timerfd` that turns readable at an absolute deadline on `CLOCK_MONOTONIC`, the
    /// clock of [`hypha_time::steady_now`].
    #[derive(Debug)]
    pub(in crate::waitset) struct DeadlineTimer {
        timer: OwnedFd,
        /// Names `timer` to the reactor; it does not close it.
        handle: FileDescriptor,
    }

    impl DeadlineTimer {
        pub(in crate::waitset) fn new() -> Result<Self, PortError> {
            let flags = TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK;
            let timer = timerfd_create(TimerfdClockId::Monotonic, flags)
                .map_err(|_| PortError::Backend(32))?;
            let handle =
                FileDescriptor::non_owning_new(timer.as_raw_fd()).ok_or(PortError::Backend(32))?;
            Ok(Self { timer, handle })
        }

        /// Attach the timer to the run's `waitset`, so that its expiry ends a wait.
        pub(in crate::waitset) fn attach<'w>(
            &'w self,
            waitset: &'w WaitSet<Ipc>,
        ) -> Result<Option<WaitSetGuard<'w, 'w, Ipc>>, PortError> {
            waitset.attach_notification(self).map(Some).map_err(|_| PortError::Backend(33))
        }

        /// Expire at `deadline`, or never with `None`. Setting the timer also clears an
        /// expiry that no wait has consumed, so the timer never ends a wait early.
        pub(in crate::waitset) fn arm(&self, deadline: Option<Time>) -> Result<(), PortError> {
            let it_value = deadline.map_or(DISARMED, expiry);
            let value = Itimerspec { it_interval: DISARMED, it_value };
            timerfd_settime(&self.timer, TimerfdTimerFlags::ABSTIME, &value)
                .map(drop)
                .map_err(|_| PortError::Backend(34))
        }
    }

    impl FileDescriptorBased for DeadlineTimer {
        fn file_descriptor(&self) -> &FileDescriptor {
            &self.handle
        }
    }

    impl SynchronousMultiplexing for DeadlineTimer {}

    /// `deadline` as an absolute expiry. One at or before the clock's origin has passed:
    /// it expires at once, at the first nanosecond, since zero would disarm the timer.
    fn expiry(deadline: Time) -> Timespec {
        let nanos = deadline.nanos().max(1);
        Timespec { tv_sec: nanos / NANOS_PER_SECOND, tv_nsec: nanos % NANOS_PER_SECOND }
    }
}

/// Without a timer of its own, the timeout of the native wait is the wake-up.
#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub(super) struct DeadlineTimer;

#[cfg(not(target_os = "linux"))]
#[expect(
    clippy::unnecessary_wraps,
    clippy::unused_self,
    reason = "the same calls as the Linux timer's, which hold a file descriptor and can fail"
)]
impl DeadlineTimer {
    pub(super) const fn new() -> Result<Self, PortError> {
        Ok(Self)
    }

    pub(super) const fn attach<'w>(
        &'w self,
        _waitset: &'w WaitSet<Ipc>,
    ) -> Result<Option<WaitSetGuard<'w, 'w, Ipc>>, PortError> {
        Ok(None)
    }

    pub(super) const fn arm(&self, _deadline: Option<Time>) -> Result<(), PortError> {
        Ok(())
    }
}
