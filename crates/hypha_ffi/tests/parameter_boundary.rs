//! Reading parameters through the C ABI: absent, malformed, or the stored kind.
#![allow(clippy::expect_used)]

mod common;
use crate::common::{config_value, create_node_with_file, stored_text, unique};

use std::ffi::CString;
use std::ptr;

use hypha_ffi::*;

fn create_node(name: &str, parameters: &str) -> *mut HyphaNode {
    let path = std::env::temp_dir().join(unique("parameters.toml"));
    std::fs::write(&path, parameters).expect("write parameter fixture");
    let node = create_node_with_file(name, Some(&path));
    std::fs::remove_file(&path).expect("remove parameter fixture");
    node
}

#[test]
fn invalid_configuration_is_rejected_before_transport_startup() {
    let name = CString::new(unique("invalid_config")).expect("node name");
    let path = std::env::temp_dir().join(unique("invalid_parameters.toml"));
    std::fs::write(&path, "broken = [\n").expect("write invalid parameter fixture");
    let path_text = CString::new(path.to_string_lossy().as_bytes()).expect("parameter path");
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
    std::fs::remove_file(path).expect("remove invalid parameter fixture");
    assert_eq!(status, HyphaStatus::InvalidArgument);
    assert!(node.is_null());
}

#[test]
fn every_scalar_kind_is_returned_as_stored() {
    let node = create_node(
        &unique("good"),
        "flag=true\nsmall=-7\nbig=-9000000000\nratio=0.5\nprecise=0.25\nwhole_float=5.0\n",
    );
    let config = unsafe { hypha_node_config(node) };

    let (result, flag) = config_value(config, "flag");
    assert_eq!(
        (result, flag.kind, flag.bool_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_BOOL, 1)
    );

    let (result, small) = config_value(config, "small");
    assert_eq!(
        (result, small.kind, small.int_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_INT, -7)
    );

    let (result, big) = config_value(config, "big");
    assert_eq!(
        (result, big.kind, big.int_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_INT, -9_000_000_000)
    );

    let (result, ratio) = config_value(config, "ratio");
    assert_eq!(
        (result, ratio.kind, ratio.float_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_FLOAT, 0.5)
    );

    let (result, precise) = config_value(config, "precise");
    assert_eq!(
        (result, precise.kind, precise.float_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_FLOAT, 0.25)
    );

    // A float written as a whole number in TOML is still a float in the value.
    let (result, whole) = config_value(config, "whole_float");
    assert_eq!(
        (result, whole.kind, whole.float_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_FLOAT, 5.0)
    );

    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_value_of_another_kind_is_returned_as_stored_not_converted() {
    let node =
        create_node("kinds", "flag='perhaps'\ncount=-1\nratio=\"one point five\"\nquoted=\"5\"\n");
    let config = unsafe { hypha_node_config(node) };

    let (result, flag) = config_value(config, "flag");
    assert_eq!((result, flag.kind), (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_TEXT));
    assert_eq!(stored_text(&flag), b"perhaps");

    let (result, count) = config_value(config, "count");
    assert_eq!(
        (result, count.kind, count.int_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_INT, -1)
    );

    let (result, ratio) = config_value(config, "ratio");
    assert_eq!((result, ratio.kind), (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_TEXT));

    // Text that looks like a number stays text.
    let (result, quoted) = config_value(config, "quoted");
    assert_eq!((result, quoted.kind), (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_TEXT));

    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_parameter_nobody_set_is_absent_and_writes_nothing() {
    let node = create_node(&unique("absent"), "configured=true\n");
    let name = "never_set";
    let mut value = crate::common::empty_config_value();
    value.kind = 99;
    let result = unsafe {
        hypha_config_value(
            hypha_node_config(node),
            name.as_ptr().cast(),
            name.len(),
            &raw mut value,
        )
    };
    assert_eq!(result, HyphaParamResult::Absent);
    assert_eq!(value.kind, 99, "an absent read writes nothing");

    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_text_parameter_borrows_its_bytes_from_the_configuration() {
    let node = create_node(&unique("text"), "greeting='hello world'\n");
    let config = unsafe { hypha_node_config(node) };

    let (result, greeting) = config_value(config, "greeting");
    assert_eq!(
        (result, greeting.kind, greeting.text_len),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_TEXT, 11)
    );
    assert_eq!(stored_text(&greeting), b"hello world");
    let (absent, _) = config_value(config, "absent_text");
    assert_eq!(absent, HyphaParamResult::Absent, "a parameter nobody set is absent");

    unsafe { hypha_node_destroy(node) };
}

#[test]
fn standalone_config_reads_parameters_and_reports_unread_keys() {
    let path = std::env::temp_dir().join(unique("standalone_config.toml"));
    std::fs::write(&path, "rate = 25\n[ctrl]\nkp = 1.5\nunused = 2.0\n").expect("write fixture");
    let path_str = path.to_string_lossy();
    let mut config: *mut HyphaConfig = ptr::null_mut();
    let status =
        unsafe { hypha_config_create(path_str.as_ptr().cast(), path_str.len(), &raw mut config) };
    std::fs::remove_file(&path).expect("remove fixture");
    assert_eq!(status, HyphaStatus::Ok);
    assert!(!config.is_null());

    let (read_status, rate) = config_value(config, "rate");
    assert_eq!(
        (read_status, rate.kind, rate.int_value),
        (HyphaParamResult::Ok, HYPHA_CONFIG_VALUE_INT, 25)
    );

    let mut len = 0_usize;
    assert_eq!(
        unsafe {
            hypha_config_unread_keys(config, ptr::null(), 0, ptr::null_mut(), 0, &raw mut len)
        },
        HyphaStatus::TooLarge
    );
    let mut buf = vec![0_u8; len];
    assert_eq!(
        unsafe {
            hypha_config_unread_keys(
                config,
                ptr::null(),
                0,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &raw mut len,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(buf.as_slice(), b"ctrl.kp\nctrl.unused");

    unsafe { hypha_config_destroy(config) };
}
