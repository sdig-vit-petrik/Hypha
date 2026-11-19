//! Setting and removing environment variables in a test.
//!
//! `std::env::set_var` and `remove_var` are `unsafe`: a write races with any other
//! thread that reads or writes the environment. Call these only before the test starts
//! such a thread or after joining it, one test per process (`cargo nextest`).
#![allow(unsafe_code, reason = "std::env::set_var and remove_var are unsafe since edition 2024")]

use std::ffi::OsStr;

/// Set the environment variable `name` to `value` for this test process.
pub fn set_env(name: impl AsRef<OsStr>, value: impl AsRef<OsStr>) {
    // SAFETY: called before the test starts a thread, or after joining it; one test per process.
    unsafe { std::env::set_var(name, value) };
}

/// Remove the environment variable `name` from this test process.
pub fn remove_env(name: impl AsRef<OsStr>) {
    // SAFETY: called before the test starts a thread, or after joining it; one test per process.
    unsafe { std::env::remove_var(name) };
}
