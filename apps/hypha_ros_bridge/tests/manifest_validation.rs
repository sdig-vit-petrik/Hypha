//! Manifest validation tests.

use hypha_ros_bridge::{BridgeDirection, BridgeError, BridgeManifest, BridgeRoute};

const STD_STRING_MD5: &str = "992ce8a1687cec8c8bd883ec73ca41d1";
const STD_STRING_DEFINITION: &str = "string data\n";

fn route(
    ros_type: Option<&str>,
    ros_md5: Option<&str>,
    ros_definition: Option<&str>,
) -> BridgeRoute {
    BridgeRoute {
        hypha_topic: "/bridge/text".into(),
        ros_topic: "/ros/text".into(),
        ros_type: ros_type.map(str::to_owned),
        ros_md5: ros_md5.map(str::to_owned),
        ros_definition: ros_definition.map(str::to_owned),
        direction: BridgeDirection::Bidirectional,
        latched: false,
    }
}

#[test]
fn a_correctly_declared_route_is_accepted_for_any_type_name() {
    // The bridge carries opaque bytes: an unfamiliar type name is fine as
    // long as the declared md5 matches its own definition text.
    let manifest = BridgeManifest {
        routes: vec![route(
            Some("migration_msgs/Custom"),
            Some(STD_STRING_MD5),
            Some(STD_STRING_DEFINITION),
        )],
    };
    assert!(manifest.validate().is_ok());
}

#[test]
fn malformed_toml_is_reported_as_configuration_error() {
    let result = BridgeManifest::parse("routes = [");
    assert!(
        matches!(result, Err(BridgeError::Config(message)) if message.contains("parse manifest"))
    );
}

#[test]
fn a_route_with_empty_definition_is_rejected_before_network_setup() {
    let manifest = BridgeManifest {
        routes: vec![route(Some("std_msgs/String"), Some(STD_STRING_MD5), Some(""))],
    };
    let result = manifest.validate();
    assert!(
        matches!(result, Err(BridgeError::Config(message)) if message.contains("ros_definition"))
    );
}

#[test]
fn a_route_with_a_malformed_md5_is_rejected() {
    let manifest = BridgeManifest {
        routes: vec![route(Some("std_msgs/String"), Some("not-hex"), Some(STD_STRING_DEFINITION))],
    };
    let result = manifest.validate();
    assert!(
        matches!(result, Err(BridgeError::Config(message)) if message.contains("malformed ros_md5"))
    );
}

#[test]
fn a_declared_md5_that_does_not_match_the_definition_is_rejected() {
    let wrong_md5 = "0".repeat(32);
    let manifest = BridgeManifest {
        routes: vec![route(Some("std_msgs/String"), Some(&wrong_md5), Some(STD_STRING_DEFINITION))],
    };
    let result = manifest.validate();
    assert!(
        matches!(result, Err(BridgeError::Config(message)) if message.contains("does not match its ros_definition"))
    );
}

#[test]
fn uppercase_md5_is_normalized_for_the_hypha_schema_identity() {
    let manifest = BridgeManifest {
        routes: vec![route(
            Some("std_msgs/String"),
            Some(&STD_STRING_MD5.to_uppercase()),
            Some(STD_STRING_DEFINITION),
        )],
    };
    let result = manifest.validate();
    assert!(result.is_ok());
    let Some(validated) = result.ok() else {
        return;
    };
    let Some(first_route) = validated.routes.first() else {
        return;
    };
    assert_eq!(first_route.ros_md5.as_deref(), Some(STD_STRING_MD5));
}

#[test]
fn malformed_dependency_section_is_reported_without_panicking() {
    let manifest = BridgeManifest {
        routes: vec![route(
            Some("migration_msgs/Custom"),
            Some(STD_STRING_MD5),
            Some(
                "string data\n================================================================================\nMSG: \n",
            ),
        )],
    };
    let result = manifest.validate();
    assert!(
        matches!(result, Err(BridgeError::Config(message)) if message.contains("ros_definition"))
    );
}

#[test]
fn missing_ros_identity_is_allowed_for_ros_to_hypha_routes() {
    let mut route = route(None, None, None);
    route.direction = BridgeDirection::Ros1ToHypha;
    let manifest = BridgeManifest { routes: vec![route] };
    let validated = manifest.validate();
    assert!(validated.is_ok());
    let Some(validated) = validated.ok() else {
        return;
    };
    let Some(route) = validated.routes.first() else {
        return;
    };
    assert_eq!(route.effective_ros_type(), "ros1/opaque");
    assert_eq!(route.effective_ros_md5(), "00000000000000000000000000000000");
    assert_eq!(route.effective_ros_definition(), "");
}

#[test]
fn a_route_giving_only_some_ros_identity_fields_is_rejected() {
    // A route that names a ros_type but forgets ros_md5 and ros_definition
    // almost certainly meant a real identity. Silently discarding ros_type
    // and reporting it as opaque would mask that mistake, so it must be
    // rejected rather than reinterpreted as a deliberately opaque route.
    let mut route = route(Some("std_msgs/String"), None, None);
    route.direction = BridgeDirection::Ros1ToHypha;
    let manifest = BridgeManifest { routes: vec![route] };
    let result = manifest.validate();
    assert!(
        matches!(result, Err(BridgeError::Config(message)) if message.contains("only some of ros_type"))
    );
}

#[test]
fn missing_ros_identity_is_rejected_for_hypha_to_ros_routes() {
    let mut route = route(None, None, None);
    route.direction = BridgeDirection::HyphaToRos1;
    let manifest = BridgeManifest { routes: vec![route] };
    let result = manifest.validate();
    assert!(
        matches!(result, Err(BridgeError::Config(message)) if message.contains("ros_type") && message.contains("ros_definition"))
    );
}

#[test]
fn a_route_declares_its_own_latching_rather_than_being_guessed_from_its_name() {
    let toml = format!(
        r#"
        [[routes]]
        hypha_topic = "/bridge/text"
        ros_topic = "/ros/text"
        ros_type = "std_msgs/String"
        ros_md5 = "{STD_STRING_MD5}"
        ros_definition = "string data\n"
        direction = "bidirectional"
        latched = true
        "#
    );
    let manifest = BridgeManifest::parse(&toml);
    assert!(manifest.is_ok(), "manifest with a declared latched field must parse: {manifest:?}");
    let Ok(manifest) = manifest else {
        return;
    };
    let Some(route) = manifest.routes.first() else {
        return;
    };
    assert!(route.latched, "the declared latched field must be honored, not the topic name");
}

#[test]
fn latched_defaults_to_false_when_not_declared() {
    let toml = format!(
        r#"
        [[routes]]
        hypha_topic = "/bridge/text"
        ros_topic = "/ros/text"
        ros_type = "std_msgs/String"
        ros_md5 = "{STD_STRING_MD5}"
        ros_definition = "string data\n"
        direction = "bidirectional"
        "#
    );
    let manifest = BridgeManifest::parse(&toml);
    assert!(manifest.is_ok(), "manifest without a declared latched field must still parse");
    let Ok(manifest) = manifest else {
        return;
    };
    let Some(route) = manifest.routes.first() else {
        return;
    };
    assert!(!route.latched);
}
