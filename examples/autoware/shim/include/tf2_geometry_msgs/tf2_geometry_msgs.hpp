#pragma once

/// @file
/// The conversions of tf2_geometry_msgs (geometry2 jazzy) that the ported code calls,
/// between the vendored tf2 LinearMath types and hypha's messages. Upstream's header needs
/// tf2's convert machinery and KDL.

#include <tf2/LinearMath/Quaternion.hpp>
#include <tf2/LinearMath/Vector3.hpp>

#include <geometry_msgs/msg/quaternion.hpp>
#include <geometry_msgs/msg/vector3.hpp>

namespace tf2 {

inline geometry_msgs::msg::Quaternion toMsg(const tf2::Quaternion& in)
{
    geometry_msgs::msg::Quaternion out;
    out.x = in.getX();
    out.y = in.getY();
    out.z = in.getZ();
    out.w = in.getW();
    return out;
}

inline void fromMsg(const geometry_msgs::msg::Quaternion& in, tf2::Quaternion& out)
{
    out = tf2::Quaternion(in.x, in.y, in.z, in.w);
}

inline geometry_msgs::msg::Vector3 toMsg(const tf2::Vector3& in)
{
    geometry_msgs::msg::Vector3 out;
    out.x = in.x();
    out.y = in.y();
    out.z = in.z();
    return out;
}

inline void fromMsg(const geometry_msgs::msg::Vector3& in, tf2::Vector3& out)
{
    out = tf2::Vector3(in.x, in.y, in.z);
}

}  // namespace tf2
