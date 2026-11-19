//! Public topic name validation across the C ABI.
#![allow(clippy::expect_used)]

use std::os::raw::c_char;

use hypha_ffi::*;

fn validate(topic: &str) -> HyphaStatus {
    unsafe { hypha_topic_validate_public_name(topic.as_ptr().cast::<c_char>(), topic.len()) }
}

#[test]
fn canonical_names_are_accepted() {
    for name in ["/a", "/atot/horizont", "/a/b_c/D~9"] {
        assert_eq!(validate(name), HyphaStatus::Ok, "{name:?} must be accepted");
    }
}

#[test]
fn relative_and_malformed_names_are_refused() {
    let refused = ["", "/", "node/example", "a", "/hypha", "/a//b", "/a/../b", "/a/*", "/a b"];
    for name in refused {
        assert_eq!(validate(name), HyphaStatus::InvalidTopic, "{name:?} must be refused");
    }
}

#[test]
fn non_utf8_name_is_an_invalid_argument() {
    let bytes = [b'/', 0xff, 0xfe];
    let status = unsafe { hypha_topic_validate_public_name(bytes.as_ptr().cast::<c_char>(), 3) };
    assert_eq!(status, HyphaStatus::InvalidArgument);
}
