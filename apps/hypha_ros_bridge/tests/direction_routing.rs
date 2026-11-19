//! Direction routing tests.

use hypha_ros_bridge::BridgeDirection;
use hypha_ros_bridge::routing::BridgeFlow;

#[test]
fn ros_to_hypha_routes_only_from_ros_enabled_directions() {
    assert!(BridgeDirection::Ros1ToHypha.allows(BridgeFlow::Ros1ToHypha));
    assert!(!BridgeDirection::Ros1ToHypha.allows(BridgeFlow::HyphaToRos1));
    assert!(BridgeDirection::Bidirectional.allows(BridgeFlow::Ros1ToHypha));
}

#[test]
fn hypha_to_ros_routes_only_from_hypha_enabled_directions() {
    assert!(!BridgeDirection::HyphaToRos1.allows(BridgeFlow::Ros1ToHypha));
    assert!(BridgeDirection::HyphaToRos1.allows(BridgeFlow::HyphaToRos1));
    assert!(BridgeDirection::Bidirectional.allows(BridgeFlow::HyphaToRos1));
}
