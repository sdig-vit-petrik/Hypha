/// @file
/// The upstream VoxelGridDownsampleFilterNode on the bus, through the rclcpp shim: its
/// parameters from TOML, an XYZIRC cloud in, one centroid per voxel out.

#include "voxel_grid_downsample_filter/voxel_grid_downsample_filter_node.hpp"

#include <gtest/gtest.h>
#include <rclcpp/rclcpp.hpp>

#include <sensor_msgs/point_cloud2_iterator.hpp>

#include <algorithm>
#include <array>
#include <cstdint>
#include <memory>
#include <optional>
#include <vector>

namespace {

constexpr const char* kParameterFile = "test/config/voxel_grid_downsample_filter_node_test.toml";

struct Point
{
    float   x;
    float   y;
    float   z;
    uint8_t intensity;
};

/// The XYZIRC layout of Autoware's sensing pipeline.
sensor_msgs::msg::PointCloud2 makeCloud(const std::vector<Point>& points)
{
    sensor_msgs::msg::PointCloud2 cloud;
    cloud.header.frame_id = "base_link";
    cloud.header.stamp.secs = 7;
    cloud.header.stamp.nsecs = 5;
    sensor_msgs::PointCloud2Modifier modifier(cloud);
    modifier.setPointCloud2Fields(
        6, "x", 1, sensor_msgs::msg::PointField::FLOAT32, "y", 1,
        sensor_msgs::msg::PointField::FLOAT32, "z", 1, sensor_msgs::msg::PointField::FLOAT32,
        "intensity", 1, sensor_msgs::msg::PointField::UINT8, "return_type", 1,
        sensor_msgs::msg::PointField::UINT8, "channel", 1, sensor_msgs::msg::PointField::UINT16
    );
    modifier.resize(points.size());
    sensor_msgs::PointCloud2Iterator<float>   x(cloud, "x");
    sensor_msgs::PointCloud2Iterator<float>   y(cloud, "y");
    sensor_msgs::PointCloud2Iterator<float>   z(cloud, "z");
    sensor_msgs::PointCloud2Iterator<uint8_t> intensity(cloud, "intensity");
    for (const Point& p : points) {
        *x = p.x;
        *y = p.y;
        *z = p.z;
        *intensity = p.intensity;
        ++x;
        ++y;
        ++z;
        ++intensity;
    }
    cloud.height = 1;
    cloud.is_dense = true;
    return cloud;
}

std::vector<Point> pointsOf(const sensor_msgs::msg::PointCloud2& cloud)
{
    std::vector<Point>                             points;
    sensor_msgs::PointCloud2ConstIterator<float>   x(cloud, "x");
    sensor_msgs::PointCloud2ConstIterator<float>   y(cloud, "y");
    sensor_msgs::PointCloud2ConstIterator<float>   z(cloud, "z");
    sensor_msgs::PointCloud2ConstIterator<uint8_t> intensity(cloud, "intensity");
    for (; x != x.end(); ++x, ++y, ++z, ++intensity) {
        points.push_back({*x, *y, *z, *intensity});
    }
    std::sort(points.begin(), points.end(), [](const Point& a, const Point& b) {
        return a.x < b.x;
    });
    return points;
}

}  // namespace

TEST(VoxelGridDownsampleFilterNode, OneCentroidPerVoxel)
{
    const char* argv[] = {"test", kParameterFile};
    rclcpp::init(2, argv);

    auto filter = std::make_shared<autoware::downsample_filters::VoxelGridDownsampleFilterNode>(
        rclcpp::NodeOptions()
    );
    EXPECT_TRUE(hypha::config().unreadKeys().empty());

    auto probe = rclcpp::Node::make_shared("voxel_grid_probe");
    std::optional<sensor_msgs::msg::PointCloud2> received;
    auto input = probe->create_publisher<sensor_msgs::msg::PointCloud2>(
        "/test/voxel_grid/input", rclcpp::SensorDataQoS()
    );
    auto output = probe->create_subscription<sensor_msgs::msg::PointCloud2>(
        "/test/voxel_grid/output", rclcpp::SensorDataQoS(),
        [&](const sensor_msgs::msg::PointCloud2& cloud) { received = cloud; }
    );

    // Three points in the voxel at the origin, two in the voxel at x 5.
    input->publish(makeCloud({
        {0.1F, 0.1F, 0.1F, 90},
        {0.2F, 0.2F, 0.2F, 100},
        {0.9F, 0.9F, 0.9F, 110},
        {5.2F, 0.5F, 0.5F, 10},
        {5.4F, 0.5F, 0.5F, 30},
    }));
    for (int slice = 0; slice < 100 && !received.has_value(); ++slice) {
        ASSERT_TRUE(probe->hypha_node().runFor(hypha::Duration::fromMillis(20)).ok());
    }

    ASSERT_TRUE(received.has_value()) << "no cloud on /test/voxel_grid/output";
    EXPECT_EQ(received->header.frame_id, "base_link");
    EXPECT_EQ(received->header.stamp.secs, 7U);
    EXPECT_EQ(received->header.stamp.nsecs, 5U);
    const std::vector<Point> points = pointsOf(*received);
    ASSERT_EQ(points.size(), 2U);
    EXPECT_NEAR(points[0].x, 0.4F, 1e-4F);
    EXPECT_NEAR(points[0].y, 0.4F, 1e-4F);
    EXPECT_EQ(points[0].intensity, 100U);
    EXPECT_NEAR(points[1].x, 5.3F, 1e-4F);
    EXPECT_NEAR(points[1].z, 0.5F, 1e-4F);
    EXPECT_EQ(points[1].intensity, 20U);
}

int main(int argc, char** argv)
{
    testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
