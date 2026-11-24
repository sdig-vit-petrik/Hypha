#pragma once

/// @file
/// The service policies of the Autoware port, for the message types whose Autoware
/// subscribers ask for a deeper queue than the `default` policy's 32 (docs/cpp-api.md §7).
/// The ROS 2 include path of each type includes this header, so every ported node
/// declares the same policy on its topics. A player or a bridge creates a topic with
/// `default`: start the nodes first.

#include <hypha/qos.h>

#include <geometry_msgs/TwistWithCovarianceStamped.h>
#include <nav_msgs/Odometry.h>

namespace hypha_rclcpp::policies {

/// The vehicle's twist and odometry: concatenate-and-time-sync subscribes them with
/// `rclcpp::QoS{100}`, a second of history for its motion compensation.
inline constexpr hypha::Qos twist{
    .name = "twist", .publishers = 8, .subscribers = 16, .maxDepth = 100
};

}  // namespace hypha_rclcpp::policies

template <>
struct hypha::QosOf<geometry_msgs::TwistWithCovarianceStamped>
{
    static constexpr hypha::Qos value = hypha_rclcpp::policies::twist;
};

template <>
struct hypha::QosOf<nav_msgs::Odometry>
{
    static constexpr hypha::Qos value = hypha_rclcpp::policies::twist;
};
