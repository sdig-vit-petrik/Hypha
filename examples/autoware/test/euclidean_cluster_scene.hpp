#pragma once

/// @file
/// The scene of the euclidean cluster node tests and their probe: two clusters of 25
/// points, one of 100 points above the tests' max_cluster_size of 50, and three lone
/// points below min_cluster_size. The probe publishes the scene and collects the node's
/// objects, debug cloud and diagnostics.

#include <autoware_perception_msgs/msg/detected_objects.hpp>
#include <diagnostic_msgs/msg/diagnostic_array.hpp>
#include <gtest/gtest.h>
#include <pcl/point_cloud.h>
#include <pcl/point_types.h>
#include <pcl_conversions/pcl_conversions.h>
#include <rclcpp/rclcpp.hpp>
#include <sensor_msgs/msg/point_cloud2.hpp>

#include <array>
#include <cmath>
#include <memory>
#include <optional>
#include <string>
#include <vector>

namespace euclidean_cluster_test {

/// The centre of a square of points in the xy plane.
struct Square
{
    float x;
    float y;
    int   side;  ///< points per side, 0.1 m apart
};

/// Two clusters to find; the big one exceeds max_cluster_size.
inline constexpr std::array<Square, 2> kClusters{{{5.0F, 0.0F, 5}, {-4.0F, 3.0F, 5}}};
inline constexpr Square                kBigCluster{0.0F, -6.0F, 10};
/// Lone points, far from everything: each is a cluster of one.
inline constexpr std::array<std::array<float, 2>, 3> kNoise{
    {{12.0F, 12.0F}, {-12.0F, 12.0F}, {12.0F, -12.0F}}
};

inline void addSquare(pcl::PointCloud<pcl::PointXYZ>& cloud, const Square& square)
{
    const float half = 0.05F * static_cast<float>(square.side - 1);
    for (int i = 0; i < square.side; ++i) {
        for (int j = 0; j < square.side; ++j) {
            cloud.push_back(
                pcl::PointXYZ(
                    square.x - half + 0.1F * static_cast<float>(i),
                    square.y - half + 0.1F * static_cast<float>(j),
                    0.5F
                )
            );
        }
    }
}

inline sensor_msgs::msg::PointCloud2 scene()
{
    pcl::PointCloud<pcl::PointXYZ> cloud;
    for (const Square& square : kClusters) {
        addSquare(cloud, square);
    }
    addSquare(cloud, kBigCluster);
    for (const auto& [x, y] : kNoise) {
        cloud.push_back(pcl::PointXYZ(x, y, 0.5F));
    }
    sensor_msgs::msg::PointCloud2 message;
    pcl::toROSMsg(cloud, message);
    message.header.frame_id = "base_link";
    message.header.stamp.secs = 42;
    return message;
}

/// What the node sent for one scene.
struct Received
{
    std::optional<autoware_perception_msgs::msg::DetectedObjects> objects;
    std::optional<sensor_msgs::msg::PointCloud2>                  clusters;
    std::optional<diagnostic_msgs::msg::DiagnosticStatus>         status;
};

/// Publishes the scene on `/test/<name>/input` and collects the outputs of the node
/// named @p rosName, whose diagnostics are `<rosName>: euclidean_cluster` on
/// `/diagnostics`; other tests of the host may publish there too.
inline Received runScene(const std::string& name, const std::string& rosName)
{
    auto              probe = rclcpp::Node::make_shared(name + "_probe");
    auto              received = std::make_shared<Received>();
    const std::string topic = "/test/" + name;
    auto              input = probe->create_publisher<sensor_msgs::msg::PointCloud2>(
        topic + "/input", rclcpp::SensorDataQoS()
    );
    auto objects = probe->create_subscription<autoware_perception_msgs::msg::DetectedObjects>(
        topic + "/output",
        rclcpp::QoS{1},
        [received](const autoware_perception_msgs::msg::DetectedObjects& message) {
            received->objects = message;
        }
    );
    auto clusters = probe->create_subscription<sensor_msgs::msg::PointCloud2>(
        topic + "/debug/clusters",
        rclcpp::QoS{1},
        [received](const sensor_msgs::msg::PointCloud2& message) {
            received->clusters = message;
        }
    );
    const std::string statusName = rosName + ": euclidean_cluster";
    auto diagnostics = probe->create_subscription<diagnostic_msgs::msg::DiagnosticArray>(
        "/diagnostics",
        rclcpp::QoS(10),
        [received, statusName](const diagnostic_msgs::msg::DiagnosticArray& message) {
            for (const auto& status : message.status) {
                if (status.name == statusName) {
                    received->status = status;
                }
            }
        }
    );

    input->publish(scene());
    for (int slice = 0; slice < 100; ++slice) {
        if (received->objects && received->clusters && received->status) {
            break;
        }
        EXPECT_TRUE(probe->hypha_node().runFor(hypha::Duration::fromMillis(20)).ok());
    }
    return *received;
}

inline geometry_msgs::msg::Point centre(const Square& square)
{
    geometry_msgs::msg::Point point;
    point.x = square.x;
    point.y = square.y;
    point.z = 0.5;
    return point;
}

/// The two clusters are the objects, in any order, at the centres of their squares; no
/// object for the big cluster or the lone points.
inline void expectTheTwoClusters(const autoware_perception_msgs::msg::DetectedObjects& objects)
{
    EXPECT_EQ(objects.header.frame_id, "base_link");
    EXPECT_EQ(objects.header.stamp.secs, 42U);
    ASSERT_EQ(objects.objects.size(), kClusters.size());
    for (const Square& square : kClusters) {
        const geometry_msgs::msg::Point expected = centre(square);
        int                             found = 0;
        for (const auto& object : objects.objects) {
            const auto& position = object.kinematics.pose_with_covariance.pose.position;
            if (std::hypot(position.x - expected.x, position.y - expected.y) < 0.5) {
                ++found;
                EXPECT_NEAR(position.x, expected.x, 1e-4);
                EXPECT_NEAR(position.y, expected.y, 1e-4);
                EXPECT_NEAR(position.z, expected.z, 1e-4);
                ASSERT_EQ(object.classification.size(), 1U);
                EXPECT_EQ(
                    object.classification[0].label,
                    autoware_perception_msgs::msg::ObjectClassification::UNKNOWN
                );
                EXPECT_FLOAT_EQ(object.classification[0].probability, 1.0F);
            }
        }
        EXPECT_EQ(found, 1) << "one object at (" << expected.x << ", " << expected.y << ")";
    }
}

/// The debug cloud holds the points of the two clusters, coloured.
inline void expectTheDebugCloud(const sensor_msgs::msg::PointCloud2& cloud)
{
    EXPECT_EQ(cloud.header.frame_id, "base_link");
    EXPECT_EQ(cloud.width, 2U * 25U);
    ASSERT_EQ(cloud.fields.size(), 4U);
    EXPECT_EQ(cloud.fields[3].name, "rgb");
}

inline std::string
value(const diagnostic_msgs::msg::DiagnosticStatus& status, const std::string& key)
{
    for (const auto& entry : status.values) {
        if (entry.key == key) {
            return entry.value;
        }
    }
    return "<missing>";
}

}  // namespace euclidean_cluster_test
