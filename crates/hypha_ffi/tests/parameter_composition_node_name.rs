//! A node takes its `node.name` from a file that the parameter file includes.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

mod common;
use crate::common::{create_node_with_file, unique};

use std::os::raw::c_char;

use hypha_ffi::*;

fn node_name(node: *const HyphaNode) -> String {
    let mut buffer = [0 as c_char; 128];
    let mut length = 0_usize;
    let status =
        unsafe { hypha_node_name(node, buffer.as_mut_ptr(), buffer.len(), &raw mut length) };
    assert_eq!(status, HyphaStatus::Ok);
    let bytes: Vec<u8> = buffer[..length].iter().map(|byte| byte.to_ne_bytes()[0]).collect();
    String::from_utf8(bytes).expect("node name is text")
}

#[test]
fn node_name_comes_from_an_included_file() {
    let dir = std::env::temp_dir().join(unique("composition_node_name"));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    std::fs::write(dir.join("name.toml"), "[node]\nname = \"composed_from_include\"\n")
        .expect("write included file");
    let root = dir.join("root.toml");
    std::fs::write(&root, "include = [\"name.toml\"]\n").expect("write root file");

    let node = create_node_with_file(&unique("code_name"), Some(&root));
    assert_eq!(node_name(node), "composed_from_include");
    unsafe { hypha_node_destroy(node) };
    std::fs::remove_dir_all(&dir).expect("remove fixture directory");
}
