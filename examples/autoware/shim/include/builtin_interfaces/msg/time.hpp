#pragma once

/// @file
/// `builtin_interfaces::msg::Time` and `Duration` are hypha's ROS 1 time types
/// (`secs`, `nsecs`) until ROS 2 definitions (#45).

#include <hypha_msgs/ros_time.h>

namespace builtin_interfaces::msg {
using Time = hypha_msgs::RosTime;
using Duration = hypha_msgs::RosDuration;
}  // namespace builtin_interfaces::msg
