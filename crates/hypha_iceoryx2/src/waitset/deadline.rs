//! The wake-up at the dispatcher's timed deadline.
//!
//! The native wait takes a relative timeout. The Linux reactor hands it to `epoll_wait`
//! rounded up to a whole millisecond, so a wait for its timeout alone ends up to 1 ms after
//! the deadline. On Linux a `timerfd` on `CLOCK_MONOTONIC`, armed with the absolute deadline
//! and attached as a notification, ends the wait at the deadline itself. A kernel without
//! `timerfd` is not supported: the `WaitSet` then fails to create. Elsewhere the timeout is
//! the wake-up: macOS's `select` takes microseconds, and the kernel's timer coalescing
//! defers its wake-up by up to half the wait.

use hypha_core::port::PortError;
use hypha_time::Time;
use iceoryx2::waitset::{WaitSet, WaitSetGuard};

use crate::transport::Ipc;

#[cfg(target_os = "linux")]
pub(in crate::waitset) use linux::DeadlineTimer;

#[cfg(target_os = "linux")]
mod linux {
    use core::cell::Cell;
    use std::os::fd::{AsRawFd, OwnedFd};

    use iceoryx2_bb_posix::file_descriptor::{FileDescriptor, FileDescriptorBased};
    use iceoryx2_bb_posix::file_descriptor_set::SynchronousMultiplexing;
    use rustix::time::{
        Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec, timerfd_create,
        timerfd_settime,
    };

    use super::{Ipc, PortError, Time, WaitSet, WaitSetGuard};

    /// Backend code of a deadline timer (`timerfd`) that could not be created.
    const BACKEND_TIMER_CREATE: u32 = 42;
    /// Backend code of a deadline timer the native `WaitSet` refused to attach.
    const BACKEND_TIMER_ATTACH: u32 = 43;
    /// Backend code of a deadline timer that could not be set.
    const BACKEND_TIMER_SET: u32 = 44;

    const NANOS_PER_SECOND: i64 = 1_000_000_000;

    /// A zero interval: the timer expires once, at its deadline.
    const ONE_SHOT: Timespec = Timespec { tv_sec: 0, tv_nsec: 0 };

    /// A zero expiry disarms a `timerfd`.
    const NEVER: Timespec = Timespec { tv_sec: 0, tv_nsec: 0 };

    /// A `timerfd` that turns readable at an absolute deadline on `CLOCK_MONOTONIC`, the
    /// clock of [`hypha_time::steady_now`].
    #[derive(Debug)]
    pub(in crate::waitset) struct DeadlineTimer {
        timer: OwnedFd,
        /// Names `timer` to the reactor; it does not close it.
        handle: FileDescriptor,
        /// Whether the last setting gave a deadline. A disarmed timer that stays
        /// disarmed is not set again, so a `WaitSet` without a timed deadline makes no
        /// system call for it.
        armed: Cell<bool>,
    }

    impl DeadlineTimer {
        pub(in crate::waitset) fn new() -> Result<Self, PortError> {
            let flags = TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK;
            let timer = timerfd_create(TimerfdClockId::Monotonic, flags).map_err(|cause| {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: the WaitSet's deadline timer (timerfd_create) failed: {cause}");
                PortError::Backend(BACKEND_TIMER_CREATE)
            })?;
            let handle = FileDescriptor::non_owning_new(timer.as_raw_fd()).ok_or_else(|| {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: the WaitSet's deadline timer has no valid descriptor");
                PortError::Backend(BACKEND_TIMER_CREATE)
            })?;
            Ok(Self { timer, handle, armed: Cell::new(false) })
        }

        /// Attach the timer to the run's `waitset`, so that its expiry ends a wait.
        pub(in crate::waitset) fn attach<'w>(
            &'w self,
            waitset: &'w WaitSet<Ipc>,
        ) -> Result<Option<WaitSetGuard<'w, 'w, Ipc>>, PortError> {
            waitset.attach_notification(self).map(Some).map_err(|cause| {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: attaching the deadline timer to the WaitSet failed: {cause:?}");
                PortError::Backend(BACKEND_TIMER_ATTACH)
            })
        }

        /// Expire at `deadline`, or never with `None`. Setting the timer also clears an
        /// expiry that no wait has consumed, so the timer never ends a wait early.
        pub(in crate::waitset) fn arm(&self, deadline: Option<Time>) -> Result<(), PortError> {
            if deadline.is_none() && !self.armed.get() {
                return Ok(());
            }
            let it_value = deadline.map_or(NEVER, expiry);
            let value = Itimerspec { it_interval: ONE_SHOT, it_value };
            timerfd_settime(&self.timer, TimerfdTimerFlags::ABSTIME, &value).map_err(|cause| {
                log::error!(target: "hypha/transport", "hypha_iceoryx2: setting the WaitSet's deadline timer (timerfd_settime) failed: {cause}");
                PortError::Backend(BACKEND_TIMER_SET)
            })?;
            self.armed.set(deadline.is_some());
            Ok(())
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
    const fn expiry(deadline: Time) -> Timespec {
        let nanos = if deadline.nanos() < 1 { 1 } else { deadline.nanos() };
        Timespec { tv_sec: nanos / NANOS_PER_SECOND, tv_nsec: nanos % NANOS_PER_SECOND }
    }

    #[cfg(test)]
    #[expect(
        clippy::expect_used,
        reason = "a failed timer call should stop the test with its message"
    )]
    mod tests {
        use core::time::Duration;

        use hypha_time::steady_now;
        use rustix::io::{Errno, read};

        use super::{DeadlineTimer, Time, Timespec, expiry};

        /// Whether the timer expired since it was last set: a read of its count, which
        /// fails with `EAGAIN` while it has not.
        fn expired(timer: &DeadlineTimer) -> bool {
            let mut count = [0_u8; 8];
            read(&timer.timer, &mut count).map_or_else(
                |cause| {
                    assert_eq!(cause, Errno::AGAIN, "reading the timer failed");
                    false
                },
                |_| true,
            )
        }

        /// Set the timer to `deadline` and report whether it expired within 2 ms.
        fn expires(timer: &DeadlineTimer, deadline: Option<Time>) -> bool {
            timer.arm(deadline).expect("set the timer");
            std::thread::sleep(Duration::from_millis(2));
            expired(timer)
        }

        #[test]
        fn a_deadline_that_passed_expires_at_once_and_none_never() {
            let timer = DeadlineTimer::new().expect("create the timer");
            for passed in [0, -5, i64::MIN, 1, 1_000_000_000] {
                assert!(expires(&timer, Some(Time::from_nanos(passed))), "deadline {passed} ns");
            }
            assert!(expires(&timer, Some(steady_now())), "a deadline of now");
            assert!(!expires(&timer, Some(Time::from_nanos(i64::MAX))), "the last deadline");
            assert!(!expires(&timer, None), "no deadline");
        }

        #[test]
        fn setting_the_timer_clears_an_unread_expiry() {
            let timer = DeadlineTimer::new().expect("create the timer");
            timer.arm(Some(Time::from_nanos(1))).expect("set the timer");
            std::thread::sleep(Duration::from_millis(2));
            assert!(!expires(&timer, None), "disarming kept the expiry");
            timer.arm(Some(Time::from_nanos(1))).expect("set the timer");
            std::thread::sleep(Duration::from_millis(2));
            let hour_ahead = steady_now() + hypha_time::Duration::from_nanos(3_600_000_000_000);
            assert!(!expires(&timer, Some(hour_ahead)), "a later deadline kept the expiry");
        }

        #[test]
        fn an_expiry_is_never_zero() {
            for passed in [i64::MIN, -1, 0] {
                assert_eq!(expiry(Time::from_nanos(passed)), Timespec { tv_sec: 0, tv_nsec: 1 });
            }
            let last = Timespec { tv_sec: 9_223_372_036, tv_nsec: 854_775_807 };
            assert_eq!(expiry(Time::from_nanos(i64::MAX)), last);
        }
    }
}

/// Without a timer of its own, the timeout of the native wait is the wake-up.
#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub(in crate::waitset) struct DeadlineTimer;

#[cfg(not(target_os = "linux"))]
#[expect(
    clippy::unnecessary_wraps,
    clippy::unused_self,
    reason = "the same calls as the Linux timer's, which hold a file descriptor and can fail"
)]
impl DeadlineTimer {
    pub(in crate::waitset) const fn new() -> Result<Self, PortError> {
        Ok(Self)
    }

    pub(in crate::waitset) const fn attach<'w>(
        &'w self,
        _waitset: &'w WaitSet<Ipc>,
    ) -> Result<Option<WaitSetGuard<'w, 'w, Ipc>>, PortError> {
        Ok(None)
    }

    pub(in crate::waitset) const fn arm(&self, _deadline: Option<Time>) -> Result<(), PortError> {
        Ok(())
    }
}
