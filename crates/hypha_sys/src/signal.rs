//! SIGINT and SIGTERM stop every node of the process; on Linux, SIGBUS names a full
//! `/dev/shm` before it ends the process.
//!
//! A signal records a process stop ([`stop_requested`]). A handler installed before
//! keeps running after this one; without one, a second signal ends the process at once.
//! A handler installed after the node replaces this one: register yours before the node.
//! A signal the process ignores stays ignored.
//!
//! On Linux a `/dev/shm` out of pages raises SIGBUS on the first write into a new
//! segment. The handler writes one line naming the cause, chains to the earlier handler,
//! then re-raises with the default action. macOS keeps its SIGBUS, which also reports a
//! stack overflow.

use std::os::raw::{c_int, c_void};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static INSTALL: Once = Once::new();
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static EARLIER_INTERRUPT: EarlierHandler = EarlierHandler::new();
static EARLIER_TERMINATE: EarlierHandler = EarlierHandler::new();
#[cfg(target_os = "linux")]
static EARLIER_BUS: EarlierHandler = EarlierHandler::new();

/// The line written when the process takes SIGBUS.
#[cfg(target_os = "linux")]
const SHARED_MEMORY_EXHAUSTED: &[u8] =
    b"hypha: this process is ending on SIGBUS, most likely because shared memory (/dev/shm) is full; \
    free /dev/shm (stale iox2* segments, with the domain stopped) and restart it\n";

/// The signature of a handler installed with `SA_SIGINFO`.
type InfoHandler = extern "C" fn(c_int, *mut libc::siginfo_t, *mut c_void);
/// The signature of a handler installed without `SA_SIGINFO`.
type PlainHandler = extern "C" fn(c_int);

/// True once a stop of the process was requested. Nothing clears it.
pub fn stop_requested() -> bool {
    STOP_REQUESTED.load(Ordering::Acquire)
}

/// Record a stop of the process, and return whether one was recorded before.
///
/// Async-signal-safe: one atomic swap.
pub fn request_stop() -> bool {
    STOP_REQUESTED.swap(true, Ordering::AcqRel)
}

/// The handler that was in place before this one, for one signal, in lock-free atomics.
struct EarlierHandler {
    address: AtomicUsize,
    takes_info: AtomicBool,
}

impl EarlierHandler {
    const fn new() -> Self {
        Self { address: AtomicUsize::new(libc::SIG_DFL), takes_info: AtomicBool::new(false) }
    }

    fn is_function(&self) -> bool {
        let address = self.address.load(Ordering::Acquire);
        address != libc::SIG_DFL && address != libc::SIG_IGN
    }

    fn call(&self, signal: c_int, info: *mut libc::siginfo_t, context: *mut c_void) {
        let address = self.address.load(Ordering::Acquire);
        if self.takes_info.load(Ordering::Acquire) {
            // SAFETY: the stored address came from `sigaction` with `SA_SIGINFO` set, and
            // `is_function` excluded `SIG_DFL` and `SIG_IGN`.
            let handler =
                unsafe { std::mem::transmute::<libc::sighandler_t, InfoHandler>(address) };
            handler(signal, info, context);
        } else {
            // SAFETY: as above, for a handler installed without `SA_SIGINFO`.
            let handler =
                unsafe { std::mem::transmute::<libc::sighandler_t, PlainHandler>(address) };
            handler(signal);
        }
    }
}

/// Installs the handlers at the first call in the process; later calls do nothing.
pub fn install() {
    INSTALL.call_once(|| {
        install_for(libc::SIGINT, &EARLIER_INTERRUPT);
        install_for(libc::SIGTERM, &EARLIER_TERMINATE);
        #[cfg(target_os = "linux")]
        install_bus_error();
    });
}

/// The handler of `signal` now, or `None` when `sigaction` cannot read it.
fn current_action(signal: c_int) -> Option<libc::sigaction> {
    // SAFETY: `sigaction` is plain data, for which all zeroes is a valid value.
    let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: a null `act` only reads the current handler into `previous`, a live local.
    let read = unsafe { libc::sigaction(signal, std::ptr::null(), &raw mut previous) };
    (read == 0).then_some(previous)
}

/// Install `handler` for `signal` with `flags` and an empty mask. False when refused.
fn set_action(signal: c_int, handler: libc::sighandler_t, flags: c_int) -> bool {
    // SAFETY: `sigaction` is plain data, for which all zeroes is a valid value.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler;
    action.sa_flags = flags;
    // SAFETY: `sigemptyset` gets a pointer to the mask of this live local.
    unsafe { libc::sigemptyset(&raw mut action.sa_mask) };
    // SAFETY: `action` is a live, initialised local whose handler matches `flags`.
    let set = unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
    set == 0
}

/// Installs the SIGBUS handler, keeping the handler that was in place before it.
#[cfg(target_os = "linux")]
fn install_bus_error() {
    let Some(previous) = current_action(libc::SIGBUS) else {
        eprintln!(
            "hypha: cannot read the handler of SIGBUS; a full /dev/shm ends the process silently"
        );
        return;
    };
    if previous.sa_sigaction == libc::SIG_IGN {
        return;
    }
    EARLIER_BUS.address.store(previous.sa_sigaction, Ordering::Release);
    EARLIER_BUS.takes_info.store(previous.sa_flags & libc::SA_SIGINFO != 0, Ordering::Release);
    // SA_NODEFER leaves SIGBUS unblocked inside the handler, so the `raise` ends the
    // process at once.
    let handler = on_bus_error as InfoHandler as libc::sighandler_t;
    if !set_action(libc::SIGBUS, handler, libc::SA_SIGINFO | libc::SA_NODEFER) {
        eprintln!(
            "hypha: cannot install the handler of SIGBUS; a full /dev/shm ends the process silently"
        );
    }
}

/// The SIGBUS handler: one line to standard error, the earlier handler if any, then the
/// default action and the signal raised again. Async-signal-safe.
#[cfg(target_os = "linux")]
extern "C" fn on_bus_error(signal: c_int, info: *mut libc::siginfo_t, context: *mut c_void) {
    // SAFETY: the pointer and length are those of a static byte string, all `write` reads.
    let _written = unsafe {
        libc::write(
            libc::STDERR_FILENO,
            SHARED_MEMORY_EXHAUSTED.as_ptr().cast(),
            SHARED_MEMORY_EXHAUSTED.len(),
        )
    };
    if EARLIER_BUS.is_function() {
        EARLIER_BUS.call(signal, info, context);
    }
    let _default = set_action(signal, libc::SIG_DFL, 0);
    // SAFETY: `raise` takes no pointer. The default action of SIGBUS ends the process.
    unsafe { libc::raise(signal) };
    // SAFETY: `_exit` takes no pointer and runs no destructor; the fallback of the raise.
    unsafe { libc::_exit(128_i32.wrapping_add(signal)) };
}

fn install_for(signal: c_int, earlier: &EarlierHandler) {
    let Some(previous) = current_action(signal) else {
        eprintln!("hypha: cannot read the handler of signal {signal}; it does not stop a node");
        return;
    };
    if previous.sa_sigaction == libc::SIG_IGN {
        return;
    }
    earlier.address.store(previous.sa_sigaction, Ordering::Release);
    earlier.takes_info.store(previous.sa_flags & libc::SA_SIGINFO != 0, Ordering::Release);

    // `on_stop_signal` has the `SA_SIGINFO` signature the flags ask for.
    let handler = on_stop_signal as InfoHandler as libc::sighandler_t;
    if !set_action(signal, handler, libc::SA_SIGINFO | libc::SA_RESTART) {
        eprintln!("hypha: cannot install the handler of signal {signal}; it does not stop a node");
    }
}

extern "C" fn on_stop_signal(signal: c_int, info: *mut libc::siginfo_t, context: *mut c_void) {
    let repeated = request_stop();
    let earlier = if signal == libc::SIGINT { &EARLIER_INTERRUPT } else { &EARLIER_TERMINATE };
    if earlier.is_function() {
        earlier.call(signal, info, context);
        return;
    }
    if repeated {
        // SAFETY: `_exit` is async-signal-safe and takes no pointer.
        unsafe { libc::_exit(128_i32.wrapping_add(signal)) };
    }
}
