#pragma once

// The ROS 2 name of <sensor_msgs/point_field_conversion.h> (vendored from ROS 1
// common_msgs, crates/hypha-cxx/upstream/common_msgs): sensor_msgs::pointFieldTypeAsType,
// typeAsPointFieldType, readPointCloud2BufferValue and writePointCloud2BufferValue,
// the same as ROS 2's. The upstream header needs the PointField definition before it;
// this one brings it in.
#include <sensor_msgs/PointField.h>
#include <sensor_msgs/point_field_conversion.h>

#if __has_include(<sensor_msgs/msg/point_field.hpp>)
#    include <sensor_msgs/msg/point_field.hpp>
#endif
