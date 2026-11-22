#pragma once

/// @file
/// The ROS 2 include path of `hypha_msgs/nav_msgs/Odometry.h`; see
/// hypha_rclcpp/message_namespace.hpp. Its service policy is in
/// hypha_rclcpp/qos_policies.hpp.

#include <hypha_msgs/nav_msgs/Odometry.h>
#include <hypha_rclcpp/message_namespace.hpp>
#include <hypha_rclcpp/qos_policies.hpp>

#include <builtin_interfaces/msg/time.hpp>
#include <geometry_msgs/msg/twist_with_covariance_stamped.hpp>
#include <std_msgs/msg/header.hpp>

HYPHA_RCLCPP_MSG_NAMESPACE(nav_msgs)
