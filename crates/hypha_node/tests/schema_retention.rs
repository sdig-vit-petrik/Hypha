//! Rust API tests for retained schema housekeeping.

#![allow(clippy::expect_used)]

use hypha_node::sdk::BusNode;
use hypha_testkit::unique_name;

/// One named schema metric of the node. Fails the test when the node does not emit it.
fn schema_metric(node: &BusNode, name: &str) -> u64 {
    node.metrics()
        .expect("metrics")
        .into_iter()
        .find(|(metric, _)| metric == name)
        .map(|(_, value)| value)
        .expect("a schema metric")
}

#[test]
fn lookup_api_keys_definitions_by_topic_type_and_md5() {
    let publisher_node =
        BusNode::new(&unique_name("schema_key_publisher")).expect("publisher node");
    let resolver_node = BusNode::new(&unique_name("schema_key_resolver")).expect("resolver node");
    let first_topic = format!("/{}", unique_name("schema_key_first"));
    let second_topic = format!("/{}", unique_name("schema_key_second"));
    let data_type = format!("{}/Type", unique_name("schema_keys_type"));
    let first_md5 = "2f594660f93a83fc58af2a4a8e891ec2";
    let second_md5 = "0b153b21d0dae7a10c8ed92dc1a76f11";
    let _first = publisher_node
        .publish_raw(&first_topic, &data_type, first_md5, "uint32 first")
        .expect("first publisher");
    let _second = publisher_node
        .publish_raw(&second_topic, &data_type, second_md5, "uint32 second")
        .expect("second publisher");

    let mut first = None;
    let mut second = None;
    for _ in 0..100 {
        first = resolver_node
            .lookup_definition(&first_topic, &data_type, first_md5)
            .expect("first schema lookup");
        second = resolver_node
            .lookup_definition(&second_topic, &data_type, second_md5)
            .expect("second schema lookup");
        if first.is_some() && second.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(first.as_deref(), Some("uint32 first"));
    assert_eq!(second.as_deref(), Some("uint32 second"));
}

#[test]
fn lookup_api_resolves_one_hundred_independent_services() {
    let publisher_node =
        BusNode::new(&unique_name("schema_scale_publisher")).expect("publisher node");
    let resolver_node = BusNode::new(&unique_name("schema_scale_resolver")).expect("resolver node");
    let mut publishers = Vec::new();
    let mut identities = Vec::new();
    for index in 0_u16..100 {
        let topic = format!("/{}/topic_{index}", unique_name("schema_scale"));
        let data_type = format!("demo/Type{index}");
        let definition = format!("uint32 value_{index}");
        let md5 = hypha_node::sdk::md5::md5_from_definition(&data_type, &definition).expect("md5");
        let publisher = publisher_node
            .publish_raw(&topic, &data_type, &md5, &definition)
            .expect("schema publisher");
        publishers.push(publisher);
        identities.push((topic, data_type, md5, definition));
    }

    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_secs(30))
        .expect("deadline");
    let mut resolved_count = 0_usize;
    while std::time::Instant::now() < deadline {
        resolved_count = 0;
        for (topic, data_type, md5, definition) in &identities {
            let resolved = resolver_node
                .lookup_definition(topic, data_type, md5)
                .expect("scale schema lookup");
            if resolved.as_ref() == Some(definition) {
                resolved_count = resolved_count.saturating_add(1);
            }
        }
        if resolved_count == identities.len() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(resolved_count, identities.len());
    assert_eq!(publishers.len(), identities.len());
}

#[test]
fn lookup_definition_resolves_and_reports_health() {
    let publisher_node = BusNode::new(&unique_name("schema_for_pub")).expect("pub node");
    let resolver_node = BusNode::new(&unique_name("schema_for_res")).expect("res node");
    let topic = format!("/{}", unique_name("schema_for_top"));
    let data_type = format!("{}/Direct", unique_name("schema_explicit_type"));
    let md5 = "74697ed3d931f6eede8bf3a8dfeca160";
    let definition = "string text";

    assert_eq!(schema_metric(&resolver_node, "schema.invalid_records"), 0);

    let _publisher =
        publisher_node.publish_raw(&topic, &data_type, md5, definition).expect("publisher");

    let mut resolved = None;
    for _ in 0..100 {
        resolved = resolver_node.lookup_definition(&topic, &data_type, md5).expect("schema lookup");
        if resolved.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(resolved.as_deref(), Some(definition));

    assert_eq!(schema_metric(&resolver_node, "schema.invalid_records"), 0);
}
