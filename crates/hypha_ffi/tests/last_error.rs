//! `hypha_last_error` carries the reason of the last refusal on the calling thread.
#![allow(clippy::expect_used)]

mod common;
use crate::common::{c_text, create_node, unique};

use std::ffi::c_void;
use std::ptr;

use hypha_ffi::*;

extern "C" fn step(
    _image: *const HyphaCycleImage,
    _time: *const HyphaCycleTime,
    _emission: *mut HyphaCycleEmission,
    _context: *mut c_void,
) -> waitset::HyphaCallbackResult {
    waitset::HyphaCallbackResult { action: waitset::HYPHA_ACTION_CONTINUE, status: 0 }
}

fn last_error() -> String {
    let mut length = 0_usize;
    let sized = unsafe { hypha_last_error(ptr::null_mut(), 0, &raw mut length) };
    assert!(sized == HyphaStatus::Ok || sized == HyphaStatus::TooLarge);
    let mut buffer = vec![0_u8; length];
    assert_eq!(
        unsafe { hypha_last_error(buffer.as_mut_ptr().cast(), buffer.len(), &raw mut length) },
        HyphaStatus::Ok
    );
    buffer.truncate(length);
    String::from_utf8(buffer).expect("UTF-8")
}

#[test]
fn a_commit_refused_for_its_schedule_records_the_reason_and_the_next_create_clears_it() {
    let node = create_node(&unique("last_error"));
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let name = c_text("no_schedule");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                0,
                Some(step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(last_error(), "");
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidArgument);
    assert_eq!(last_error(), "cycle 'no_schedule' is refused: it has neither a rate nor a trigger");

    // A too-small buffer reports the length and leaves the buffer alone.
    let mut small = [0x7f_u8; 4];
    let mut length = 0_usize;
    assert_eq!(
        unsafe { hypha_last_error(small.as_mut_ptr().cast(), small.len(), &raw mut length) },
        HyphaStatus::TooLarge
    );
    assert_eq!(length, last_error().len());
    assert_eq!(small, [0x7f; 4]);
    assert_eq!(
        unsafe { hypha_last_error(ptr::null_mut(), 0, ptr::null_mut()) },
        HyphaStatus::InvalidArgument
    );

    let other = c_text("next");
    let mut next = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                other.as_ptr(),
                other.as_bytes().len(),
                1_000_000,
                Some(step),
                ptr::null_mut(),
                None,
                &raw mut next,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(last_error(), "");
    unsafe { hypha_node_destroy(node) };
}

#[test]
fn a_cycle_on_a_cli_tool_node_is_refused_with_the_reason() {
    let node_name = format!("_hyphacli_{}", unique("last_error_tool"));
    let node = create_node(&node_name);
    let mut waitset = ptr::null_mut();
    assert_eq!(unsafe { hypha_node_default_waitset(node, &raw mut waitset) }, HyphaStatus::Ok);
    let name = c_text("tool_cycle");
    let mut cycle = ptr::null_mut();
    assert_eq!(
        unsafe {
            hypha_cycle_create(
                waitset,
                name.as_ptr(),
                name.as_bytes().len(),
                1_000_000,
                Some(step),
                ptr::null_mut(),
                None,
                &raw mut cycle,
            )
        },
        HyphaStatus::Ok
    );
    assert_eq!(unsafe { hypha_cycle_commit(cycle) }, HyphaStatus::InvalidState);
    assert_eq!(
        last_error(),
        format!("cycle 'tool_cycle' is refused: the CLI tool node '{node_name}' runs no cycle")
    );
    unsafe { hypha_node_destroy(node) };
}
