//! Integration tests for `hypha_msgs`.

use hypha_core::message::{DecodeOwned, EncodeInto};
use hypha_msgs::geometry_msgs::{Point, Pose, PoseStamped, Quaternion};
use hypha_msgs::sensor_msgs::{Image, PointCloud2, PointField};
use hypha_msgs::std_msgs::Header;
use hypha_wire::ros_time::RosTime;

#[test]
#[allow(clippy::expect_used)]
fn header_identity_and_roundtrip() {
    assert_eq!(Header::TYPE_NAME, "std_msgs/Header");
    assert_eq!(Header::SCHEMA_HASH, "2176decaecbce78abc3b96ef049fabed");

    let header = Header {
        seq: 42,
        stamp: RosTime { secs: 1_700_000_000, nsecs: 500 },
        frame_id: "base_link".into(),
    };

    let len = header.encoded_len();
    let mut buf = vec![0u8; len];
    let written = header.encode_into(&mut buf).expect("encoding header");
    assert_eq!(written, len);

    let decoded = Header::decode_from(&buf).expect("decoding header");
    assert_eq!(decoded, header);
}

#[test]
#[allow(clippy::expect_used)]
fn pose_stamped_identity_and_roundtrip() {
    assert_eq!(PoseStamped::TYPE_NAME, "geometry_msgs/PoseStamped");
    assert_eq!(PoseStamped::SCHEMA_HASH, "d3812c3cbc69362b77dc0b19b345f8f5");

    let msg = PoseStamped {
        header: Header { seq: 1, stamp: RosTime { secs: 100, nsecs: 200 }, frame_id: "map".into() },
        pose: Pose {
            position: Point { x: 1.0, y: 2.0, z: 3.0 },
            orientation: Quaternion { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
        },
    };

    let len = msg.encoded_len();
    let mut buf = vec![0u8; len];
    let written = msg.encode_into(&mut buf).expect("encoding pose stamped");
    assert_eq!(written, len);

    let decoded = PoseStamped::decode_from(&buf).expect("decoding pose stamped");
    assert_eq!(decoded, msg);
}

#[test]
#[allow(clippy::expect_used)]
fn image_identity_and_roundtrip() {
    assert_eq!(Image::TYPE_NAME, "sensor_msgs/Image");
    assert_eq!(Image::SCHEMA_HASH, "060021388200f6f0f447d0fcd9c64743");

    let img = Image {
        header: Header {
            seq: 10,
            stamp: RosTime { secs: 5, nsecs: 10 },
            frame_id: "camera".into(),
        },
        height: 480,
        width: 640,
        encoding: "rgb8".into(),
        is_bigendian: 0,
        step: 640 * 3,
        data: vec![255, 128, 64, 0, 1, 2],
    };

    let len = img.encoded_len();
    let mut buf = vec![0u8; len];
    let written = img.encode_into(&mut buf).expect("encoding image");
    assert_eq!(written, len);

    let decoded = Image::decode_from(&buf).expect("decoding image");
    assert_eq!(decoded, img);
}

#[test]
#[allow(clippy::expect_used)]
fn point_cloud2_identity_and_roundtrip() {
    assert_eq!(PointCloud2::TYPE_NAME, "sensor_msgs/PointCloud2");
    assert_eq!(PointCloud2::SCHEMA_HASH, "1158d486dd51d683ce2f1be655c3c181");

    let cloud = PointCloud2 {
        header: Header { seq: 1, stamp: RosTime { secs: 1, nsecs: 2 }, frame_id: "lidar".into() },
        height: 1,
        width: 2,
        fields: vec![
            PointField { name: "x".into(), offset: 0, datatype: PointField::FLOAT32, count: 1 },
            PointField { name: "y".into(), offset: 4, datatype: PointField::FLOAT32, count: 1 },
        ],
        is_bigendian: false,
        point_step: 8,
        row_step: 16,
        data: vec![0; 16],
        is_dense: true,
    };

    let len = cloud.encoded_len();
    let mut buf = vec![0u8; len];
    let written = cloud.encode_into(&mut buf).expect("encoding point cloud");
    assert_eq!(written, len);

    let decoded = PointCloud2::decode_from(&buf).expect("decoding point cloud");
    assert_eq!(decoded, cloud);
}
