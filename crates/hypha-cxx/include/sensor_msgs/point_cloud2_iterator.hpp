#pragma once

// The ROS 2 name of <sensor_msgs/point_cloud2_iterator.h> (vendored from ROS 1
// common_msgs, crates/hypha-cxx/upstream/common_msgs). The classes are the same
// sensor_msgs::PointCloud2Modifier, PointCloud2Iterator and PointCloud2ConstIterator
// ROS 2 declares, working on the one PointCloud2 type that sensor_msgs::msg::PointCloud2
// names.
#include <sensor_msgs/point_cloud2_iterator.h>

// ROS 2's header brings in the message; so does this one, once the application has
// generated the ROS 2 names of sensor_msgs (hypha_generate_ros2_headers).
#if __has_include(<sensor_msgs/msg/point_cloud2.hpp>)
#    include <sensor_msgs/msg/point_cloud2.hpp>
#endif
#if __has_include(<sensor_msgs/msg/point_field.hpp>)
#    include <sensor_msgs/msg/point_field.hpp>
#endif
