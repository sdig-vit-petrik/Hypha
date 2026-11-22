#pragma once

/// @file
/// `builtin_interfaces::msg::Time` and `Duration` are hypha's ROS 1 time types
/// (`secs`, `nsecs`) until ROS 2 definitions (#45). They compare with `==`, as every
/// ROS 2 message does.

#include <hypha_msgs/ros_time.h>

namespace hypha_msgs {
inline bool operator==(const RosTime& a, const RosTime& b)
{
    return a.secs == b.secs && a.nsecs == b.nsecs;
}

inline bool operator==(const RosDuration& a, const RosDuration& b)
{
    return a.secs == b.secs && a.nsecs == b.nsecs;
}
}  // namespace hypha_msgs

namespace builtin_interfaces::msg {
using Time = hypha_msgs::RosTime;
using Duration = hypha_msgs::RosDuration;
}  // namespace builtin_interfaces::msg
