//! The logging C ABI: argument checks and fault counting.
#![allow(clippy::expect_used)]

mod common;

use std::ptr;

use hypha_ffi::*;

/// The `log.*` counters, read through the process-wide metrics call (a null node).
struct Health {
    lines_written: u64,
    rejected_lines: u64,
}

fn health() -> Health {
    let log = |name: &str| common::node_metric(ptr::null(), name).expect("a log metric");
    Health { lines_written: log("log.lines_written"), rejected_lines: log("log.rejected_lines") }
}

fn register(name: &str) -> HyphaLogModule {
    let mut id: HyphaLogModule = 0;
    let status =
        unsafe { hypha_log_module_register(name.as_ptr().cast(), name.len(), &raw mut id) };
    assert_eq!(status, HyphaStatus::Ok);
    id
}

const INFO: u32 = HyphaLogLevel::Info as u32;

#[test]
fn register_refuses_invalid_names() {
    for name in ["", "bad name", "bad-name", "\u{e9}"] {
        let mut id: HyphaLogModule = 0;
        let status =
            unsafe { hypha_log_module_register(name.as_ptr().cast(), name.len(), &raw mut id) };
        assert_eq!(status, HyphaStatus::InvalidArgument, "name {name:?}");
    }
}

#[test]
fn enabled_follows_the_default_level() {
    let module = register("log_boundary/level");
    assert_eq!(hypha_log_enabled(module, INFO), 1);
    assert_eq!(hypha_log_enabled(module, HyphaLogLevel::Debug as u32), 1);
    assert_eq!(hypha_log_enabled(module, HyphaLogLevel::Trace as u32), 0);
    assert_eq!(hypha_log_enabled(module, HyphaLogLevel::Off as u32), 0);
    assert_eq!(hypha_log_enabled(module, 99), 0);
    assert_eq!(hypha_log_enabled(u32::MAX, INFO), 0);
}

#[test]
fn write_counts_lines_written() {
    let module = register("log_boundary/write");
    let before = health().lines_written;
    unsafe { hypha_log_write(module, INFO, b"hello\n".as_ptr().cast(), 6) };
    unsafe { hypha_log_write(module, INFO, b"\xff\xfe".as_ptr().cast(), 2) };
    assert_eq!(health().lines_written, before + 2);
}

#[test]
fn write_counts_rejected_lines() {
    let module = register("log_boundary/reject");
    let text = b"x".as_ptr().cast();
    let before = health().rejected_lines;
    unsafe { hypha_log_write(u32::MAX, INFO, text, 1) };
    assert_eq!(health().rejected_lines, before + 1, "unknown module");
    unsafe { hypha_log_write(module, 99, text, 1) };
    assert_eq!(health().rejected_lines, before + 2, "invalid level");
    unsafe { hypha_log_write(module, HyphaLogLevel::Off as u32, text, 1) };
    assert_eq!(health().rejected_lines, before + 3, "level off");
    unsafe { hypha_log_write(module, INFO, ptr::null(), 4) };
    assert_eq!(health().rejected_lines, before + 4, "null text with length");
}

#[test]
fn write_accepts_null_text_with_zero_length() {
    let module = register("log_boundary/empty");
    let before = health().rejected_lines;
    unsafe { hypha_log_write(module, INFO, ptr::null(), 0) };
    assert_eq!(health().rejected_lines, before);
}

/// Runs in its own process under nextest: the module table is process-wide.
#[test]
fn module_register_past_256_returns_the_default_identifier() {
    for index in 0..300_u32 {
        let _id = register(&format!("abi_fill_{index}"));
    }
    assert_eq!(register("abi_one_more"), 0, "a new name past the limit gets `default`");
    let name = "bad name";
    let mut id: HyphaLogModule = 7;
    let status =
        unsafe { hypha_log_module_register(name.as_ptr().cast(), name.len(), &raw mut id) };
    assert_eq!(status, HyphaStatus::InvalidArgument);
}
