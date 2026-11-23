#pragma once

/// @file
/// The tf2 types of autoware_agnocast_wrapper without Agnocast: upstream's Buffer is
/// tf2_ros::Buffer, and its listener and broadcaster hold the tf2_ros ones of the node.
/// Here they are the shim's (tf2_ros/buffer.hpp, tf2_ros/transform_broadcaster.hpp).

#include <autoware/agnocast_wrapper/node.hpp>
#include <tf2_ros/buffer.hpp>
#include <tf2_ros/transform_broadcaster.hpp>

namespace autoware::agnocast_wrapper {

using Buffer = tf2_ros::Buffer;
using TransformListener = tf2_ros::TransformListener;
using TransformBroadcaster = tf2_ros::TransformBroadcaster;

}  // namespace autoware::agnocast_wrapper
