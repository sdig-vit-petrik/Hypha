//! The standard services of ROS 1 (`msg/<package>/*.srv`), as Rust types: the name and MD5
//! of each, and a request and a response of one through the codec.

#![allow(clippy::expect_used)]

use hypha_core::message::{DecodeOwned, EncodeInto};
use hypha_msgs::nav_msgs::{GetMapRequest, GetMapResponse, MapMetaData, OccupancyGrid};
use hypha_msgs::sensor_msgs::{CameraInfo, SetCameraInfoRequest};
use hypha_msgs::std_msgs::Header;

#[test]
fn each_standard_service_has_the_name_and_md5_of_ros() {
    use hypha_msgs::{diagnostic_msgs, nav_msgs, sensor_msgs, std_srvs, tf2_msgs};

    let services = [
        (
            "diagnostic_msgs/AddDiagnostics",
            diagnostic_msgs::AddDiagnostics::TYPE_NAME,
            diagnostic_msgs::AddDiagnostics::MD5,
            "e6ac9bbde83d0d3186523c3687aecaee",
        ),
        (
            "diagnostic_msgs/SelfTest",
            diagnostic_msgs::SelfTest::TYPE_NAME,
            diagnostic_msgs::SelfTest::MD5,
            "ac21b1bab7ab17546986536c22eb34e9",
        ),
        (
            "nav_msgs/GetMap",
            nav_msgs::GetMap::TYPE_NAME,
            nav_msgs::GetMap::MD5,
            "6cdd0a18e0aff5b0a3ca2326a89b54ff",
        ),
        (
            "nav_msgs/GetPlan",
            nav_msgs::GetPlan::TYPE_NAME,
            nav_msgs::GetPlan::MD5,
            "421c8ea4d21c6c9db7054b4bbdf1e024",
        ),
        (
            "nav_msgs/LoadMap",
            nav_msgs::LoadMap::TYPE_NAME,
            nav_msgs::LoadMap::MD5,
            "22e647fdfbe3b23c8c9f419908afaebd",
        ),
        (
            "nav_msgs/SetMap",
            nav_msgs::SetMap::TYPE_NAME,
            nav_msgs::SetMap::MD5,
            "c36922319011e63ed7784112ad4fdd32",
        ),
        (
            "sensor_msgs/SetCameraInfo",
            sensor_msgs::SetCameraInfo::TYPE_NAME,
            sensor_msgs::SetCameraInfo::MD5,
            "bef1df590ed75ed1f393692395e15482",
        ),
        (
            "tf2_msgs/FrameGraph",
            tf2_msgs::FrameGraph::TYPE_NAME,
            tf2_msgs::FrameGraph::MD5,
            "437ea58e9463815a0d511c7326b686b0",
        ),
        (
            "std_srvs/Trigger",
            std_srvs::Trigger::TYPE_NAME,
            std_srvs::Trigger::MD5,
            "937c9679a518e3a18d831e57125ea522",
        ),
    ];
    for (expected_name, name, md5, expected_md5) in services {
        assert_eq!(name, expected_name);
        assert_eq!(md5, expected_md5, "{name}");
    }
}

#[test]
fn a_get_map_answer_and_a_set_camera_info_request_round_trip() {
    let request = GetMapRequest {};
    assert_eq!(request.encoded_len(), 0);

    let response = GetMapResponse {
        map: OccupancyGrid {
            header: Header { frame_id: "map".into(), ..Header::default() },
            info: MapMetaData { width: 2, height: 2, resolution: 0.05, ..MapMetaData::default() },
            data: vec![0, 100, -1, 0],
        },
    };
    let mut buffer = vec![0u8; response.encoded_len()];
    response.encode_into(&mut buffer).expect("the answer encodes");
    assert_eq!(GetMapResponse::decode_from(&buffer).expect("the answer decodes"), response);

    let camera =
        SetCameraInfoRequest { camera_info: CameraInfo { width: 640, ..CameraInfo::default() } };
    let mut buffer = vec![0u8; camera.encoded_len()];
    camera.encode_into(&mut buffer).expect("the request encodes");
    assert_eq!(SetCameraInfoRequest::decode_from(&buffer).expect("it decodes"), camera);
}
