//! A signal handler installed before any node still runs, and an ignored stop signal
//! stays ignored. All but the first test run in a child process: this binary, run again.
#![allow(clippy::expect_used)]

use std::os::raw::{c_int, c_void};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

mod common;
use crate::common::{create_node, unique};

use hypha_ffi::{hypha_node_destroy, hypha_node_ok};

static EARLIER_HANDLER_CALLS: AtomicU32 = AtomicU32::new(0);

extern "C" fn earlier_handler(_signal: libc::c_int) {
    EARLIER_HANDLER_CALLS.fetch_add(1, Ordering::SeqCst);
}

fn raise(signal: libc::c_int) {
    assert_eq!(unsafe { libc::raise(signal) }, 0);
}

#[test]
fn a_handler_installed_before_the_node_still_runs() {
    let previous =
        unsafe { libc::signal(libc::SIGINT, earlier_handler as *const () as libc::sighandler_t) };
    assert_ne!(previous, libc::SIG_ERR);
    let node = create_node(&unique("stop_signal_early"));
    raise(libc::SIGINT);
    assert_eq!(EARLIER_HANDLER_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(unsafe { hypha_node_ok(node) }, 0);
    unsafe { hypha_node_destroy(node) };
}

/// Set in the child process; names the test the child runs.
const CHILD_VARIABLE: &str = "HYPHA_STOP_SIGNAL_CHILD";
/// What a child prints once every check of its test passed.
const CHILD_PASSED: &str = "hypha stop-signal child: every check passed";

fn is_child_of(test: &str) -> bool {
    std::env::var_os(CHILD_VARIABLE).is_some_and(|name| name == test)
}

/// Run this test binary again as the child of `test`; it must exit 0 and print
/// `CHILD_PASSED`, so a filter that ran no test fails.
fn run_child(test: &str) {
    let output: Output = Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", test, "--nocapture", "--test-threads", "1"])
        .env(CHILD_VARIABLE, test)
        .output()
        .expect("run the child process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the child passes: {:?}\n{stdout}\n{stderr}", output.status);
    assert!(stdout.contains(CHILD_PASSED), "the child ran its checks:\n{stdout}\n{stderr}");
}

/// What the three-argument handler saw for one signal.
struct InfoRecord {
    calls: AtomicU32,
    /// `si_signo` of the `siginfo_t` it got, -1 when the pointer was null.
    signal_number: AtomicI32,
    /// Whether the third argument, the `ucontext_t`, was non-null.
    context_given: AtomicBool,
}

impl InfoRecord {
    const fn new() -> Self {
        Self {
            calls: AtomicU32::new(0),
            signal_number: AtomicI32::new(0),
            context_given: AtomicBool::new(false),
        }
    }
}

static INFO_INTERRUPT: InfoRecord = InfoRecord::new();
static INFO_TERMINATE: InfoRecord = InfoRecord::new();

/// A `SA_SIGINFO` handler: records what the kernel's three arguments held.
extern "C" fn earlier_info_handler(
    signal: c_int,
    info: *mut libc::siginfo_t,
    context: *mut c_void,
) {
    let record = if signal == libc::SIGINT { &INFO_INTERRUPT } else { &INFO_TERMINATE };
    // SAFETY: a non-null `info` is the `siginfo_t` the kernel filled for this signal.
    let signal_number = unsafe { info.as_ref() }.map_or(-1, |info| info.si_signo);
    record.signal_number.store(signal_number, Ordering::SeqCst);
    record.context_given.store(!context.is_null(), Ordering::SeqCst);
    record.calls.fetch_add(1, Ordering::SeqCst);
}

fn install_info_handler(signal: c_int) {
    // SAFETY: `sigaction` is plain data, for which all zeroes is a valid value.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    let handler: extern "C" fn(c_int, *mut libc::siginfo_t, *mut c_void) = earlier_info_handler;
    action.sa_sigaction = handler as libc::sighandler_t;
    action.sa_flags = libc::SA_SIGINFO;
    // SAFETY: the mask of a live local.
    unsafe { libc::sigemptyset(&raw mut action.sa_mask) };
    // SAFETY: `action` is a live, initialised local with a `SA_SIGINFO` handler.
    let set = unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
    assert_eq!(set, 0, "install the earlier handler of signal {signal}");
}

fn current_handler(signal: c_int) -> libc::sighandler_t {
    // SAFETY: `sigaction` is plain data, for which all zeroes is a valid value.
    let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: a null `act` only reads the handler into `current`, a live local.
    let read = unsafe { libc::sigaction(signal, std::ptr::null(), &raw mut current) };
    assert_eq!(read, 0, "read the handler of signal {signal}");
    current.sa_sigaction
}

const INFO_TEST: &str = "a_siginfo_handler_installed_before_the_node_gets_its_three_arguments";

#[test]
fn a_siginfo_handler_installed_before_the_node_gets_its_three_arguments() {
    if !is_child_of(INFO_TEST) {
        run_child(INFO_TEST);
        return;
    }
    install_info_handler(libc::SIGINT);
    install_info_handler(libc::SIGTERM);
    let node = create_node(&unique("stop_signal_siginfo"));
    raise(libc::SIGINT);
    raise(libc::SIGTERM);
    for (signal, record) in [(libc::SIGINT, &INFO_INTERRUPT), (libc::SIGTERM, &INFO_TERMINATE)] {
        assert_eq!(record.calls.load(Ordering::SeqCst), 1, "signal {signal}: chained once");
        assert_eq!(
            record.signal_number.load(Ordering::SeqCst),
            signal,
            "signal {signal}: the earlier handler gets the kernel's siginfo_t"
        );
        assert!(
            record.context_given.load(Ordering::SeqCst),
            "signal {signal}: the earlier handler gets the kernel's context"
        );
    }
    // SAFETY: `node` is the live node just created.
    assert_eq!(unsafe { hypha_node_ok(node) }, 0, "the signal still stops the node");
    // SAFETY: as above, destroyed once.
    unsafe { hypha_node_destroy(node) };
    println!("{CHILD_PASSED}");
}

const IGNORED_TEST: &str = "a_stop_signal_ignored_before_the_node_stays_ignored";

#[test]
fn a_stop_signal_ignored_before_the_node_stays_ignored() {
    if !is_child_of(IGNORED_TEST) {
        run_child(IGNORED_TEST);
        return;
    }
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: `signal` takes no pointer.
        let previous = unsafe { libc::signal(signal, libc::SIG_IGN) };
        assert_ne!(previous, libc::SIG_ERR);
    }
    let node = create_node(&unique("stop_signal_ignored"));
    raise(libc::SIGINT);
    raise(libc::SIGTERM);
    // The process is still running here, and neither signal reached the node.
    // SAFETY: `node` is the live node just created.
    assert_eq!(unsafe { hypha_node_ok(node) }, 1, "an ignored signal does not stop the node");
    for signal in [libc::SIGINT, libc::SIGTERM] {
        assert_eq!(current_handler(signal), libc::SIG_IGN, "signal {signal} stays ignored");
    }
    // SAFETY: as above, destroyed once.
    unsafe { hypha_node_destroy(node) };
    println!("{CHILD_PASSED}");
}
