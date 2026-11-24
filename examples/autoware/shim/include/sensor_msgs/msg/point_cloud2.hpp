#pragma once

/// @file
/// The ROS 2 include path of `sensor_msgs/PointCloud2.h`; see
/// hypha_rclcpp/message_namespace.hpp.

#include <hypha_rclcpp/message_namespace.hpp>

#include <builtin_interfaces/msg/time.hpp>
#include <sensor_msgs/PointCloud2.h>
#include <sensor_msgs/msg/point_field.hpp>

HYPHA_RCLCPP_MSG_NAMESPACE(sensor_msgs)
HYPHA_RCLCPP_MSG_ALLOCATOR_TEMPLATE(sensor_msgs, PointCloud2)
