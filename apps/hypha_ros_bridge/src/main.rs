//! `hypha_ros_bridge` binary entry point.

fn main() -> Result<(), hypha_ros_bridge::BridgeError> {
    hypha_ros_bridge::run_with_args(std::env::args())
}
