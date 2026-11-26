//! A node created without a parameter file beside a prepared configuration shares it: the
//! C++ SDK's second `Node` of a process relies on this (#208). A node that names another
//! file is refused with its cause (`node_creation_error_lines.rs`).
#![allow(clippy::expect_used, reason = "test setup must succeed")]

mod common;
use crate::common::{c_text, unique};

use std::ptr;

use hypha_ffi::*;

#[test]
fn a_node_without_a_file_shares_the_configuration_loaded_from_one() {
    let path = std::env::temp_dir().join(format!("hypha_node_shares_{}.toml", std::process::id()));
    std::fs::write(&path, "gain = 1\n").expect("write the parameter file");
    let text = path.to_str().expect("a UTF-8 path");
    let mut config = ptr::null_mut();
    // SAFETY: `text` holds `text.len()` bytes and `config` is one writable pointer.
    let loaded = unsafe { hypha_config_create(text.as_ptr().cast(), text.len(), &raw mut config) };
    assert_eq!(loaded, HyphaStatus::Ok);

    let name = c_text(&unique("node_shares"));
    let mut node = ptr::null_mut();
    // SAFETY: the name holds its length, no file is named, `config` is live and `node` is
    // one writable pointer.
    let status = unsafe {
        hypha_node_create(
            name.as_ptr(),
            name.as_bytes().len(),
            ptr::null(),
            0,
            config,
            &raw mut node,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);

    let mut buffer = vec![0_u8; 64];
    let mut length = 0_usize;
    // SAFETY: `node` is live; `buffer` has `buffer.len()` writable bytes and `length` is one
    // writable `usize`.
    let read = unsafe {
        hypha_config_unread_keys(
            hypha_node_config(node),
            ptr::null(),
            0,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &raw mut length,
        )
    };
    assert_eq!(read, HyphaStatus::Ok);
    buffer.truncate(length);
    assert_eq!(buffer, b"gain", "the node holds the file of the configuration");

    // SAFETY: `node` and `config` are live and not used afterwards.
    unsafe {
        hypha_node_destroy(node);
        hypha_config_destroy(config);
    }
    std::fs::remove_file(&path).unwrap_or_default();
}
