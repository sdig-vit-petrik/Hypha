#pragma once

/// @file
/// The conversions and transforms of tf2_geometry_msgs (geometry2 jazzy f702874) that the
/// ported code calls, between the vendored tf2 LinearMath types and hypha's messages.
/// Upstream's header needs tf2's convert machinery and KDL; a rotation here is KDL's
/// Rotation::Quaternion (orocos_kdl frames.cpp), so a quaternion that is not quite unit
/// rotates as it does in ROS.

#include <geometry_msgs/msg/quaternion.hpp>
#include <geometry_msgs/msg/transform_stamped.hpp>
#include <geometry_msgs/msg/vector3.hpp>
#include <geometry_msgs/msg/vector3_stamped.hpp>
#include <tf2/LinearMath/Quaternion.hpp>
#include <tf2/LinearMath/Vector3.hpp>

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

/// @p t_in rotated by the rotation of @p transform; the translation does not apply to a
/// vector.
inline void doTransform(
    const geometry_msgs::msg::Vector3&          t_in,
    geometry_msgs::msg::Vector3&                t_out,
    const geometry_msgs::msg::TransformStamped& transform
)
{
    const auto&  q = transform.transform.rotation;
    const double x2 = q.x * q.x;
    const double y2 = q.y * q.y;
    const double z2 = q.z * q.z;
    const double w2 = q.w * q.w;
    const double x = t_in.x;
    const double y = t_in.y;
    const double z = t_in.z;
    t_out.x = (w2 + x2 - y2 - z2) * x + (2 * q.x * q.y - 2 * q.w * q.z) * y
            + (2 * q.x * q.z + 2 * q.w * q.y) * z;
    t_out.y = (2 * q.x * q.y + 2 * q.w * q.z) * x + (w2 - x2 + y2 - z2) * y
            + (2 * q.y * q.z - 2 * q.w * q.x) * z;
    t_out.z = (2 * q.x * q.z - 2 * q.w * q.y) * x + (2 * q.y * q.z + 2 * q.w * q.x) * y
            + (w2 - x2 - y2 + z2) * z;
}

/// As above, in the frame and at the time of @p transform.
inline void doTransform(
    const geometry_msgs::msg::Vector3Stamped&   t_in,
    geometry_msgs::msg::Vector3Stamped&         t_out,
    const geometry_msgs::msg::TransformStamped& transform
)
{
    doTransform(t_in.vector, t_out.vector, transform);
    t_out.header.stamp = transform.header.stamp;
    t_out.header.frame_id = transform.header.frame_id;
}

}  // namespace tf2
