//! A process that takes SIGBUS because `/dev/shm` is full says why, then ends on SIGBUS.
//! The test raises SIGBUS in a child process: this test binary, run again.

#![allow(clippy::expect_used)]

mod common;
use crate::common::{create_node, unique};

use std::os::unix::process::ExitStatusExt as _;
use std::process::{Command, Output};

const CHILD_VARIABLE: &str = "HYPHA_BUS_ERROR_CHILD";
const TEST_NAME: &str = "a_bus_error_names_the_full_shared_memory_and_ends_on_sigbus";
const EARLIER_TEST_NAME: &str = "a_bus_error_calls_the_handler_installed_before_the_node";
/// The value of `CHILD_VARIABLE` that makes the child install a handler of its own first.
const WITH_EARLIER_HANDLER: &str = "earlier";
const EARLIER_LINE: &[u8] = b"earlier SIGBUS handler ran\n";
/// The value of `CHILD_VARIABLE` that makes the child install a `SA_SIGINFO` handler first.
const WITH_EARLIER_INFO_HANDLER: &str = "earlier-info";
/// What the `SA_SIGINFO` handler writes when it got the kernel's `siginfo_t` for SIGBUS.
const EARLIER_INFO_LINE: &[u8] = b"earlier SIGBUS handler ran with its siginfo_t and context\n";
/// What it writes when either pointer was missing or named another signal.
const EARLIER_INFO_MISSING_LINE: &[u8] = b"earlier SIGBUS handler ran without its siginfo_t\n";
const EARLIER_INFO_TEST_NAME: &str =
    "a_bus_error_calls_the_siginfo_handler_installed_before_the_node_with_its_three_arguments";
const IGNORING: &str = "ignored";
const IGNORED_TEST_NAME: &str = "a_bus_error_ignored_before_the_node_stays_ignored";
/// What the ignoring child prints once it is past the raised SIGBUS.
const IGNORED_CHILD_PASSED: &str = "hypha bus-error child: still running after an ignored SIGBUS";

const HYPHA_LINE: &str = "ending on SIGBUS, most likely because shared memory (/dev/shm) is full";

/// The body of the child: a node, then SIGBUS. Without the handler it returns and the
/// parent's status check fails.
fn child() {
    let node = create_node(&unique("bus_error"));
    assert!(!node.is_null());
    // SAFETY: `raise` takes no pointer; the handler the node installed ends the process.
    unsafe { libc::raise(libc::SIGBUS) };
}

/// A SIGBUS handler the process installs before any node: one line, and it returns.
extern "C" fn earlier_handler(_signal: libc::c_int) {
    // SAFETY: a static byte string; `write` is async-signal-safe.
    let _written = unsafe {
        libc::write(libc::STDERR_FILENO, EARLIER_LINE.as_ptr().cast(), EARLIER_LINE.len())
    };
}

/// A `SA_SIGINFO` SIGBUS handler installed before any node: one line, and it returns.
extern "C" fn earlier_info_handler(
    signal: libc::c_int,
    info: *mut libc::siginfo_t,
    context: *mut libc::c_void,
) {
    // SAFETY: a non-null `info` is the `siginfo_t` the kernel filled for this signal.
    let signal_number = unsafe { info.as_ref() }.map(|info| info.si_signo);
    let complete =
        signal == libc::SIGBUS && signal_number == Some(libc::SIGBUS) && !context.is_null();
    let line = if complete { EARLIER_INFO_LINE } else { EARLIER_INFO_MISSING_LINE };
    // SAFETY: a static byte string; `write` is async-signal-safe.
    let _written = unsafe { libc::write(libc::STDERR_FILENO, line.as_ptr().cast(), line.len()) };
}

fn install_earlier_info_handler() {
    // SAFETY: `sigaction` is plain data, for which all zeroes is a valid value.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    let handler: extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void) =
        earlier_info_handler;
    action.sa_sigaction = handler as libc::sighandler_t;
    action.sa_flags = libc::SA_SIGINFO;
    // SAFETY: the mask of a live local.
    unsafe { libc::sigemptyset(&raw mut action.sa_mask) };
    // SAFETY: `action` is a live, initialised local with a `SA_SIGINFO` handler.
    let set = unsafe { libc::sigaction(libc::SIGBUS, &raw const action, std::ptr::null_mut()) };
    assert_eq!(set, 0, "install the earlier SIGBUS handler");
}

fn current_bus_handler() -> libc::sighandler_t {
    // SAFETY: `sigaction` is plain data, for which all zeroes is a valid value.
    let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: a null `act` only reads the handler into `current`, a live local.
    let read = unsafe { libc::sigaction(libc::SIGBUS, std::ptr::null(), &raw mut current) };
    assert_eq!(read, 0, "read the handler of SIGBUS");
    current.sa_sigaction
}

/// Run this test binary again as the child of `test`, with `CHILD_VARIABLE` set to `mode`.
fn run_child(test: &str, mode: &str) -> Output {
    Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", test, "--nocapture", "--test-threads", "1"])
        .env(CHILD_VARIABLE, mode)
        .output()
        .expect("run the child process")
}

#[test]
fn a_bus_error_names_the_full_shared_memory_and_ends_on_sigbus() {
    if std::env::var_os(CHILD_VARIABLE).is_some() {
        child();
        return;
    }
    let output = run_child(TEST_NAME, "1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.signal(),
        Some(libc::SIGBUS),
        "the child ends by SIGBUS, not by a status: {:?}\n{stderr}",
        output.status
    );
    assert!(stderr.contains(HYPHA_LINE), "the child names the cause on standard error: {stderr}");
}

#[test]
fn a_bus_error_calls_the_handler_installed_before_the_node() {
    if std::env::var_os(CHILD_VARIABLE).is_some_and(|mode| mode == WITH_EARLIER_HANDLER) {
        let handler = earlier_handler as *const () as libc::sighandler_t;
        // SAFETY: `signal` takes no pointer; `earlier_handler` has the signature it calls.
        let previous = unsafe { libc::signal(libc::SIGBUS, handler) };
        assert_ne!(previous, libc::SIG_ERR);
        child();
        return;
    }
    let output = run_child(EARLIER_TEST_NAME, WITH_EARLIER_HANDLER);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.signal(),
        Some(libc::SIGBUS),
        "the earlier handler returns, and the child still ends by SIGBUS: {:?}\n{stderr}",
        output.status
    );
    let line = stderr.find(HYPHA_LINE).expect("the hypha line is on standard error");
    let earlier_text = std::str::from_utf8(EARLIER_LINE).expect("the line is text");
    let earlier = stderr.find(earlier_text).expect("the earlier handler ran");
    assert!(line < earlier, "the hypha line comes first, then the earlier handler: {stderr}");
}

#[test]
fn a_bus_error_calls_the_siginfo_handler_installed_before_the_node_with_its_three_arguments() {
    if std::env::var_os(CHILD_VARIABLE).is_some_and(|mode| mode == WITH_EARLIER_INFO_HANDLER) {
        install_earlier_info_handler();
        child();
        return;
    }
    let output = run_child(EARLIER_INFO_TEST_NAME, WITH_EARLIER_INFO_HANDLER);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.signal(),
        Some(libc::SIGBUS),
        "the earlier handler returns, and the child still ends by SIGBUS: {:?}\n{stderr}",
        output.status
    );
    let line = stderr.find(HYPHA_LINE).expect("the hypha line is on standard error");
    let earlier_text = std::str::from_utf8(EARLIER_INFO_LINE).expect("the line is text");
    let earlier = stderr.find(earlier_text);
    assert!(earlier.is_some(), "the earlier handler got its three arguments: {stderr}");
    let earlier = earlier.expect("found above");
    assert!(line < earlier, "the hypha line comes first, then the earlier handler: {stderr}");
}

#[test]
fn a_bus_error_ignored_before_the_node_stays_ignored() {
    if std::env::var_os(CHILD_VARIABLE).is_some_and(|mode| mode == IGNORING) {
        // SAFETY: `signal` takes no pointer.
        let previous = unsafe { libc::signal(libc::SIGBUS, libc::SIG_IGN) };
        assert_ne!(previous, libc::SIG_ERR);
        let node = create_node(&unique("bus_error_ignored"));
        assert!(!node.is_null());
        // SAFETY: `raise` takes no pointer; an ignored signal is discarded.
        assert_eq!(unsafe { libc::raise(libc::SIGBUS) }, 0);
        assert_eq!(current_bus_handler(), libc::SIG_IGN, "the node leaves SIGBUS ignored");
        println!("{IGNORED_CHILD_PASSED}");
        // SAFETY: `node` is the live node created above, destroyed once.
        unsafe { hypha_ffi::hypha_node_destroy(node) };
        return;
    }
    let output = run_child(IGNORED_TEST_NAME, IGNORING);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the child is not ended by an ignored SIGBUS: {:?}\n{stdout}\n{stderr}",
        output.status
    );
    assert!(stdout.contains(IGNORED_CHILD_PASSED), "the child ran its checks:\n{stdout}\n{stderr}");
    assert!(!stderr.contains(HYPHA_LINE), "no hypha handler ran: {stderr}");
}
