/// @file
/// The upstream GroundFilterComponent on the bus, through the rclcpp shim: its parameters
/// and the vehicle's from TOML, a flat ground with one obstacle in, the obstacle out.

#include "ground_filter_node.hpp"

#include <autoware/point_types/types.hpp>
#include <gtest/gtest.h>
#include <pcl_conversions/pcl_conversions.h>
#include <rclcpp/rclcpp.hpp>

#include <cmath>
#include <cstddef>
#include <memory>
#include <optional>
#include <vector>

namespace {

constexpr const char* kParameterFile = "test/config/ground_filter_node_test.toml";

using Point = autoware::point_types::PointXYZIRC;

Point point(float x, float y, float z)
{
    Point p;
    p.x = x;
    p.y = y;
    p.z = z;
    p.intensity = 50;
    p.return_type = 1;
    p.channel = 0;
    return p;
}

/// Flat ground around the vehicle, rings every 0.5 m from 3 m to 15 m, a point every degree;
/// and a pole 8 m ahead: points from 0.5 m to 1.5 m high.
pcl::PointCloud<Point> scene(std::size_t& groundCount, std::size_t& poleCount)
{
    pcl::PointCloud<Point> cloud;
    for (float radius = 3.0F; radius <= 15.0F; radius += 0.5F) {
        for (int degree = 0; degree < 360; ++degree) {
            const float a = static_cast<float>(degree) * static_cast<float>(M_PI) / 180.0F;
            cloud.push_back(point(radius * std::cos(a), radius * std::sin(a), 0.0F));
        }
    }
    groundCount = cloud.size();
    for (float z = 0.5F; z <= 1.5F; z += 0.1F) {
        cloud.push_back(point(8.0F, 0.0F, z));
        cloud.push_back(point(8.05F, 0.05F, z));
    }
    poleCount = cloud.size() - groundCount;
    return cloud;
}

}  // namespace

TEST(GroundFilterNode, RemovesTheGroundAndKeepsTheObstacle)
{
    const char* argv[] = {"test", kParameterFile};
    rclcpp::init(2, argv);

    auto filter =
        std::make_shared<autoware::ground_filter::GroundFilterComponent>(rclcpp::NodeOptions());
    EXPECT_TRUE(hypha::config().unreadKeys().empty());

    auto probe = rclcpp::Node::make_shared("ground_filter_probe");
    std::optional<sensor_msgs::msg::PointCloud2> received;
    auto input = probe->create_publisher<sensor_msgs::msg::PointCloud2>(
        "/test/ground_filter/input", rclcpp::SensorDataQoS()
    );
    auto output = probe->create_subscription<sensor_msgs::msg::PointCloud2>(
        "/test/ground_filter/output",
        rclcpp::SensorDataQoS(),
        [&](const sensor_msgs::msg::PointCloud2& cloud) {
            received = cloud;
        }
    );

    std::size_t                   groundCount = 0;
    std::size_t                   poleCount = 0;
    sensor_msgs::msg::PointCloud2 cloud;
    pcl::toROSMsg(scene(groundCount, poleCount), cloud);
    cloud.header.frame_id = "base_link";
    cloud.header.stamp.secs = 42;
    input->publish(cloud);
    for (int slice = 0; slice < 100 && !received.has_value(); ++slice) {
        ASSERT_TRUE(probe->hypha_node().runFor(hypha::Duration::fromMillis(20)).ok());
    }

    ASSERT_TRUE(received.has_value()) << "no cloud on /test/ground_filter/output";
    EXPECT_EQ(received->header.frame_id, "base_link");
    EXPECT_EQ(received->header.stamp.secs, 42U);
    pcl::PointCloud<Point> kept;
    pcl::fromROSMsg(*received, kept);
    std::size_t keptPole = 0;
    std::size_t keptGround = 0;
    for (const Point& p : kept) {
        (p.z > 0.25F ? keptPole : keptGround) += 1;
    }
    EXPECT_EQ(keptPole, poleCount) << "every obstacle point stays";
    EXPECT_EQ(keptGround, 0U) << "of " << groundCount << " ground points";
}

int main(int argc, char** argv)
{
    testing::InitGoogleTest(&argc, argv);
    return RUN_ALL_TESTS();
}
