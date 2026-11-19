//! C ABI boundary values and invalid arguments, called as a C client calls them.

mod common;
use crate::common::c_text_opt as c_text;
use crate::common::{config_value, stored_text, unique};

use std::os::raw::c_void;
use std::ptr;

use hypha_ffi::*;

unsafe fn create_node_with_parameters(name: &str, parameters: &str) -> *mut HyphaNode {
    let Some(name) = c_text(name) else { return ptr::null_mut() };
    let path = std::env::temp_dir().join(unique("parameters.toml"));
    if std::fs::write(&path, parameters).is_err() {
        return ptr::null_mut();
    }
    let Some(path_text) = path.to_str().and_then(c_text) else { return ptr::null_mut() };
    let mut node = ptr::null_mut();
    let status = unsafe {
        hypha_node_create(
            name.as_ptr(),
            name.as_bytes().len(),
            path_text.as_ptr(),
            path_text.as_bytes().len(),
            ptr::null(),
            &raw mut node,
        )
    };
    let _removed = std::fs::remove_file(path);
    assert_eq!(status, HyphaStatus::Ok);
    node
}

#[test]
fn status_and_clock_report_boundary_values() {
    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
    assert_eq!(hypha_clock_set_sim_time(1), HyphaStatus::InvalidArgument as i32);
    assert_eq!(hypha_clock_advance_sim_time(1), HyphaStatus::InvalidArgument as i32);
    assert_eq!(hypha_clock_enable_sim_time(100), HyphaStatus::Ok as i32);
    assert_eq!(hypha_clock_is_sim_time(), 1);
    assert_eq!(hypha_clock_now_nanos(), 100);
    assert_eq!(hypha_clock_set_sim_time(200), HyphaStatus::Ok as i32);
    assert_eq!(hypha_clock_advance_sim_time(30), HyphaStatus::Ok as i32);
    assert_eq!(hypha_clock_now_nanos(), 230);
    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
    assert!(hypha_clock_steady_now_nanos() >= 0);
    assert!(hypha_clock_now_nanos() > 0);
}

#[test]
fn a_negative_simulated_reading_or_span_is_refused() {
    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
    assert_eq!(hypha_clock_enable_sim_time(-1), HyphaStatus::InvalidArgument as i32);
    assert_eq!(hypha_clock_is_sim_time(), 0);
    assert_eq!(hypha_clock_enable_sim_time(100), HyphaStatus::Ok as i32);
    assert_eq!(hypha_clock_set_sim_time(-1), HyphaStatus::InvalidArgument as i32);
    assert_eq!(hypha_clock_advance_sim_time(-1), HyphaStatus::InvalidArgument as i32);
    assert_eq!(hypha_clock_now_nanos(), 100);
    assert_eq!(hypha_clock_disable_sim_time(), HyphaStatus::Ok as i32);
}

extern "C" fn count_node(_info: *const HyphaNodeInfo, user_data: *mut c_void) {
    if !user_data.is_null() {
        unsafe {
            let count = &mut *user_data.cast::<u32>();
            *count = count.saturating_add(1);
        }
    }
}

#[test]
fn node_and_parameter_apis_report_invalid_arguments() {
    assert_eq!(unsafe { hypha_node_ok(ptr::null()) }, 0);
    unsafe { hypha_node_shutdown(ptr::null_mut()) };
    unsafe { hypha_node_destroy(ptr::null_mut()) };
    let node = unsafe { create_node_with_parameters(&unique("node_errors"), "name='value'\n") };
    assert!(!node.is_null());
    let (result, text) = config_value(unsafe { hypha_node_config(node) }, "name");
    assert_eq!((result, text.kind), (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_TEXT));
    assert_eq!(stored_text(&text), b"value");
    let mut callbacks = 0;
    assert_eq!(
        unsafe { hypha_list_nodes(Some(count_node), (&raw mut callbacks).cast()) },
        HyphaStatus::Ok
    );
    assert!(callbacks > 0);
    unsafe { hypha_node_destroy(node) };
}
