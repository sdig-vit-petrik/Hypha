/// @file
/// The upstream CropBoxFilterNode on the bus, through the rclcpp shim: its parameters
/// from TOML, its static transform from /tf_static before spin, a cloud in on a hypha
/// topic, the cropped cloud out on another.

#include "crop_box_filter_node.hpp"

#include <gtest/gtest.h>
#include <hypha/transform_buffer.h>
#include <rclcpp/rclcpp.hpp>

#include <sensor_msgs/point_cloud2_iterator.hpp>

#include <array>
#include <cmath>
#include <cstring>
#include <limits>
#include <memory>
#include <optional>
#include <string>
#include <vector>

namespace {

constexpr const char* kParameterFile = "test/config/crop_box_filter_node_test.toml";

struct PointXYZI
{
    float x;
    float y;
    float z;
    float intensity;
};

sensor_msgs::msg::PointCloud2 makeCloud(const std::vector<PointXYZI>& points)
{
    sensor_msgs::msg::PointCloud2 cloud;
    cloud.header.frame_id = "lidar";
    cloud.header.stamp.secs = 100;
    cloud.header.stamp.nsecs = 250;
    sensor_msgs::PointCloud2Modifier modifier(cloud);
    modifier.setPointCloud2Fields(
        4, "x", 1, sensor_msgs::msg::PointField::FLOAT32, "y", 1,
        sensor_msgs::msg::PointField::FLOAT32, "z", 1, sensor_msgs::msg::PointField::FLOAT32,
        "intensity", 1, sensor_msgs::msg::PointField::FLOAT32
    );
    modifier.resize(points.size());
    std::memcpy(cloud.data.data(), points.data(), points.size() * sizeof(PointXYZI));
    return cloud;
}

std::vector<PointXYZI> pointsOf(const sensor_msgs::msg::PointCloud2& cloud)
{
    std::vector<PointXYZI> points;
    sensor_msgs::PointCloud2ConstIterator<float> x(cloud, "x");
    sensor_msgs::PointCloud2ConstIterator<float> y(cloud, "y");
    sensor_msgs::PointCloud2ConstIterator<float> z(cloud, "z");
    sensor_msgs::PointCloud2ConstIterator<float> intensity(cloud, "intensity");
    for (; x != x.end(); ++x, ++y, ++z, ++intensity) {
        points.push_back({*x, *y, *z, *intensity});
    }
    return points;
}

geometry_msgs::msg::TransformStamped baseToLidar()
{
    geometry_msgs::msg::TransformStamped t;
    t.header.frame_id = "base_link";
    t.child_frame_id = "lidar";
    t.transform.translation.x = 1.0;
    t.transform.rotation.w = 1.0;
    return t;
}

}  // namespace

TEST(CropBoxFilterNode, CropsInTheInputFrameAndKeepsTheOtherFields)
{
    const char* argv[] = {"test", kParameterFile};
    rclcpp::init(2, argv);

    // The probe publishes the static transform before the filter looks it up.
    auto probe = rclcpp::Node::make_shared("crop_box_probe");
    hypha::StaticTransformBroadcaster statics{probe->hypha_node()};
    ASSERT_TRUE(statics.send(baseToLidar()));

    auto filter =
        std::make_shared<autoware::crop_box_filter::CropBoxFilterNode>(rclcpp::NodeOptions());
    EXPECT_TRUE(hypha::config().unreadKeys().empty());

    std::optional<sensor_msgs::msg::PointCloud2> received;
    auto input = probe->create_publisher<sensor_msgs::msg::PointCloud2>(
        "/test/crop_box/input", rclcpp::SensorDataQoS()
    );
    auto output = probe->create_subscription<sensor_msgs::msg::PointCloud2>(
        "/test/crop_box/output", rclcpp::SensorDataQoS(),
        [&](const sensor_msgs::msg::PointCloud2::ConstPtr& cloud) { received = *cloud; }
    );

    const float nan = std::numeric_limits<float>::quiet_NaN();
    // In base_link (x + 1): 1.0 and 1.5 inside, 2.5 ahead of the box, -0.5 behind it,
    // y 3.0 beside it, and a NaN point the filter skips.
    input->publish(makeCloud({
        {0.0F, 0.0F, 0.0F, 10.0F},
        {0.5F, 0.5F, 0.5F, 20.0F},
        {1.5F, 0.0F, 0.0F, 30.0F},
        {-1.5F, 0.0F, 0.0F, 40.0F},
        {0.0F, 3.0F, 0.0F, 50.0F},
        {nan, 0.0F, 0.0F, 60.0F},
    }));
    for (int slice = 0; slice < 100 && !received.has_value(); ++slice) {
        ASSERT_TRUE(probe->hypha_node().runFor(hypha::Duration::fromMillis(20)).ok());
    }

    ASSERT_TRUE(received.has_value()) << "no cropped cloud on /test/crop_box/output";
    EXPECT_EQ(received->header.frame_id, "base_link");
    EXPECT_EQ(received->header.stamp.secs, 100U);
    EXPECT_EQ(received->header.stamp.nsecs, 250U);
    EXPECT_EQ(received->height, 1U);
    EXPECT_EQ(received->width, 2U);
    const std::vector<PointXYZI> points = pointsOf(*received);
    ASSERT_EQ(points.size(), 2U);
    EXPECT_FLOAT_EQ(points[0].x, 1.0F);
    EXPECT_FLOAT_EQ(points[0].intensity, 10.0F);
    EXPECT_FLOAT_EQ(points[1].x, 1.5F);
    EXPECT_FLOAT_EQ(points[1].y, 0.5F);
    EXPECT_FLOAT_EQ(points[1].intensity, 20.0F);
}

int main(int argc, char** argv)
{
    testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
