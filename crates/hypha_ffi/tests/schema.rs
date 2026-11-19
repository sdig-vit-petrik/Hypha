//! Tests for schema C ABI operations.

#![allow(clippy::expect_used)]

mod common;
use crate::common::{create_node, schema_counts};

use std::ptr;

use hypha_ffi::{
    HyphaNode, HyphaPublisher, HyphaPublisherOptions, HyphaStatus, hypha_node_create,
    hypha_node_destroy, hypha_publish_with_qos, hypha_publisher_destroy, hypha_schema_lookup,
};
use hypha_testkit::unique_name;

#[test]
fn schema_lookup_resolves_retained_definition() {
    let pub_node_name = unique_name("ffi_schema_pub");
    let pub_node = create_node(&pub_node_name);

    let topic = format!("/{}", unique_name("ffi_topic"));
    let data_type = "std_msgs/String";
    let md5 = "992ce8a1687cec8c8bd883ec73ca41d1";
    let definition = "string data\n";

    let mut publisher: *mut HyphaPublisher = ptr::null_mut();
    let adv_status = unsafe {
        hypha_publish_with_qos(
            pub_node,
            topic.as_ptr().cast(),
            topic.len(),
            data_type.as_ptr().cast(),
            data_type.len(),
            md5.as_ptr().cast(),
            md5.len(),
            definition.as_ptr().cast(),
            definition.len(),
            &HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() },
            &raw mut publisher,
        )
    };
    assert_eq!(adv_status, HyphaStatus::Ok);

    let res_node_name = unique_name("ffi_schema_res");
    let res_node = create_node(&res_node_name);

    let mut needed: usize = 0;
    let mut resolved = false;
    for _ in 0..40 {
        let lookup_status = unsafe {
            hypha_schema_lookup(
                res_node,
                topic.as_ptr().cast(),
                topic.len(),
                data_type.as_ptr().cast(),
                data_type.len(),
                md5.as_ptr().cast(),
                md5.len(),
                ptr::null_mut(),
                0,
                &raw mut needed,
            )
        };
        if lookup_status == HyphaStatus::Ok {
            resolved = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(resolved, "hypha_schema_lookup should find retained schema");
    assert_eq!(needed, definition.len());

    let mut buf = vec![0u8; needed];
    let mut written: usize = 0;
    let copy_status = unsafe {
        hypha_schema_lookup(
            res_node,
            topic.as_ptr().cast(),
            topic.len(),
            data_type.as_ptr().cast(),
            data_type.len(),
            md5.as_ptr().cast(),
            md5.len(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            &raw mut written,
        )
    };
    assert_eq!(copy_status, HyphaStatus::Ok);
    assert_eq!(written, definition.len());
    assert_eq!(std::str::from_utf8(&buf).ok(), Some(definition));

    let health = schema_counts(res_node);
    assert_eq!(health.invalid_records, 0);
    assert_eq!(health.resource_limit_failures, 0);

    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(pub_node);
        hypha_node_destroy(res_node);
    }
}

#[test]
fn schema_lookup_reports_not_available_on_unknown_topic() {
    let node_name = unique_name("ffi_schema_unknown");
    let mut node: *mut HyphaNode = ptr::null_mut();
    let status = unsafe {
        hypha_node_create(
            node_name.as_ptr().cast(),
            node_name.len(),
            ptr::null(),
            0,
            ptr::null(),
            &raw mut node,
        )
    };
    assert_eq!(status, HyphaStatus::Ok);

    let topic = "/unknown_topic";
    let data_type = "std_msgs/String";
    let md5 = "11111111111111111111111111111111";
    let mut needed = usize::MAX;

    let null_output_status = unsafe {
        hypha_schema_lookup(
            node,
            topic.as_ptr().cast(),
            topic.len(),
            data_type.as_ptr().cast(),
            data_type.len(),
            md5.as_ptr().cast(),
            md5.len(),
            ptr::null_mut(),
            1,
            &raw mut needed,
        )
    };
    assert_eq!(null_output_status, HyphaStatus::InvalidArgument);
    assert_eq!(needed, usize::MAX);

    let lookup_status = unsafe {
        hypha_schema_lookup(
            node,
            topic.as_ptr().cast(),
            topic.len(),
            data_type.as_ptr().cast(),
            data_type.len(),
            md5.as_ptr().cast(),
            md5.len(),
            ptr::null_mut(),
            0,
            &raw mut needed,
        )
    };
    assert_eq!(lookup_status, HyphaStatus::NotAvailable);
    assert_eq!(needed, usize::MAX);

    let health = schema_counts(node);
    assert_eq!(health.unavailable_lookups, 1);

    unsafe {
        hypha_node_destroy(node);
    }
}

#[test]
fn schema_lookup_reports_short_buffer_exactly() {
    let pub_node_name = unique_name("ffi_schema_status_pub");
    let mut pub_node: *mut HyphaNode = ptr::null_mut();
    let create_status = unsafe {
        hypha_node_create(
            pub_node_name.as_ptr().cast(),
            pub_node_name.len(),
            ptr::null(),
            0,
            ptr::null(),
            &raw mut pub_node,
        )
    };
    assert_eq!(create_status, HyphaStatus::Ok);

    let topic = format!("/{}", unique_name("ffi_status_topic"));
    let data_type = "demo/Status";
    let md5 = "d9d213e8c146065ec958333c617eb1a6";
    let definition = "uint32 value\n";
    let mut publisher: *mut HyphaPublisher = ptr::null_mut();
    let publish_status = unsafe {
        hypha_publish_with_qos(
            pub_node,
            topic.as_ptr().cast(),
            topic.len(),
            data_type.as_ptr().cast(),
            data_type.len(),
            md5.as_ptr().cast(),
            md5.len(),
            definition.as_ptr().cast(),
            definition.len(),
            &HyphaPublisherOptions { latch: 0, loaned_samples: 0, qos: crate::common::test_qos() },
            &raw mut publisher,
        )
    };
    assert_eq!(publish_status, HyphaStatus::Ok);

    let res_node_name = unique_name("ffi_schema_status_res");
    let mut res_node: *mut HyphaNode = ptr::null_mut();
    let resolver_status = unsafe {
        hypha_node_create(
            res_node_name.as_ptr().cast(),
            res_node_name.len(),
            ptr::null(),
            0,
            ptr::null(),
            &raw mut res_node,
        )
    };
    assert_eq!(resolver_status, HyphaStatus::Ok);

    let mut needed = 0_usize;
    for _ in 0..40 {
        let lookup_status = unsafe {
            hypha_schema_lookup(
                res_node,
                topic.as_ptr().cast(),
                topic.len(),
                data_type.as_ptr().cast(),
                data_type.len(),
                md5.as_ptr().cast(),
                md5.len(),
                ptr::null_mut(),
                0,
                &raw mut needed,
            )
        };
        if lookup_status == HyphaStatus::Ok {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(needed, definition.len());

    let mut short = vec![0x5a_u8; needed.saturating_sub(1)];
    let short_status = unsafe {
        hypha_schema_lookup(
            res_node,
            topic.as_ptr().cast(),
            topic.len(),
            data_type.as_ptr().cast(),
            data_type.len(),
            md5.as_ptr().cast(),
            md5.len(),
            short.as_mut_ptr().cast(),
            short.len(),
            &raw mut needed,
        )
    };
    assert_eq!(short_status, HyphaStatus::TooLarge);
    assert!(short.iter().all(|byte| *byte == 0x5a));

    unsafe {
        hypha_publisher_destroy(publisher);
        hypha_node_destroy(pub_node);
        hypha_node_destroy(res_node);
    }
}

#[test]
fn schema_lookup_reports_invalid_topic_exactly() {
    let node_name = unique_name("ffi_schema_invalid_topic");
    let mut node: *mut HyphaNode = ptr::null_mut();
    let create_status = unsafe {
        hypha_node_create(
            node_name.as_ptr().cast(),
            node_name.len(),
            ptr::null(),
            0,
            ptr::null(),
            &raw mut node,
        )
    };
    assert_eq!(create_status, HyphaStatus::Ok);

    let topic = "relative/topic";
    let data_type = "demo/Status";
    let md5 = "d9d213e8c146065ec958333c617eb1a6";
    let mut needed = usize::MAX;
    let lookup_status = unsafe {
        hypha_schema_lookup(
            node,
            topic.as_ptr().cast(),
            topic.len(),
            data_type.as_ptr().cast(),
            data_type.len(),
            md5.as_ptr().cast(),
            md5.len(),
            ptr::null_mut(),
            0,
            &raw mut needed,
        )
    };
    assert_eq!(lookup_status, HyphaStatus::InvalidTopic);
    assert_eq!(needed, usize::MAX);

    unsafe {
        hypha_node_destroy(node);
    }
}
