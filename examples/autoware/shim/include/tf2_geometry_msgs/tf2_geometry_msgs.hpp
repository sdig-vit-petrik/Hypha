#pragma once

/// @file
/// The conversions and transforms of tf2_geometry_msgs (geometry2 jazzy f702874) that the
/// ported code calls, between the vendored tf2 LinearMath types and hypha's messages.
/// Upstream's header needs tf2's convert machinery and KDL; a rotation here is KDL's
/// Rotation::Quaternion (orocos_kdl frames.cpp), so a quaternion that is not quite unit
/// rotates as it does in ROS.

#include <geometry_msgs/msg/pose.hpp>
#include <geometry_msgs/msg/quaternion.hpp>
#include <geometry_msgs/msg/transform.hpp>
#include <geometry_msgs/msg/transform_stamped.hpp>
#include <geometry_msgs/msg/vector3.hpp>
#include <geometry_msgs/msg/vector3_stamped.hpp>
#include <tf2/LinearMath/Matrix3x3.hpp>
#include <tf2/LinearMath/Quaternion.hpp>
#include <tf2/LinearMath/Transform.hpp>
#include <tf2/LinearMath/Vector3.hpp>

#include <array>
#include <utility>

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

/// geometry2's: the 6x6 pose covariance @p cov_in rotated by @p transform, block by block,
/// R * C * R^T.
inline std::array<double, 36>
transformCovariance(const std::array<double, 36>& cov_in, const tf2::Transform& transform)
{
    const tf2::Matrix3x3   R = transform.getBasis();
    const tf2::Matrix3x3   R_transpose = R.transpose();
    std::array<double, 36> cov_out{};
    // The four 3x3 blocks start at (row, column) 0/0, 0/3, 3/0 and 3/3.
    for (const auto [row0, col0] :
         {std::pair{0, 0}, std::pair{0, 3}, std::pair{3, 0}, std::pair{3, 3}}) {
        const auto at = [&](int r, int c) {
            return cov_in[static_cast<size_t>((row0 + r) * 6 + col0 + c)];
        };
        const tf2::Matrix3x3 block(
            at(0, 0), at(0, 1), at(0, 2), at(1, 0), at(1, 1), at(1, 2), at(2, 0), at(2, 1), at(2, 2)
        );
        const tf2::Matrix3x3 result = R * block * R_transpose;
        for (int r = 0; r < 3; ++r) {
            for (int c = 0; c < 3; ++c) {
                cov_out[static_cast<size_t>((row0 + r) * 6 + col0 + c)] = result[r][c];
            }
        }
    }
    return cov_out;
}

inline geometry_msgs::msg::Transform toMsg(const tf2::Transform& in)
{
    geometry_msgs::msg::Transform out;
    out.translation.x = in.getOrigin().getX();
    out.translation.y = in.getOrigin().getY();
    out.translation.z = in.getOrigin().getZ();
    out.rotation = toMsg(in.getRotation());
    return out;
}

inline void toMsg(const tf2::Transform& in, geometry_msgs::msg::Transform& out)
{
    out = toMsg(in);
}

inline void fromMsg(const geometry_msgs::msg::Transform& in, tf2::Transform& out)
{
    out.setOrigin(tf2::Vector3(in.translation.x, in.translation.y, in.translation.z));
    out.setRotation(tf2::Quaternion(in.rotation.x, in.rotation.y, in.rotation.z, in.rotation.w));
}

inline void toMsg(const tf2::Transform& in, geometry_msgs::msg::Pose& out)
{
    out.position.x = in.getOrigin().getX();
    out.position.y = in.getOrigin().getY();
    out.position.z = in.getOrigin().getZ();
    out.orientation = toMsg(in.getRotation());
}

inline void fromMsg(const geometry_msgs::msg::Pose& in, tf2::Transform& out)
{
    out.setOrigin(tf2::Vector3(in.position.x, in.position.y, in.position.z));
    out.setRotation(
        tf2::Quaternion(in.orientation.x, in.orientation.y, in.orientation.z, in.orientation.w)
    );
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

/// @p t_in moved by @p transform: geometry2 composes the two as KDL frames; here as tf2
/// transforms, the same for a unit quaternion.
inline void doTransform(
    const geometry_msgs::msg::Pose&             t_in,
    geometry_msgs::msg::Pose&                   t_out,
    const geometry_msgs::msg::TransformStamped& transform
)
{
    tf2::Transform frame;
    tf2::Transform pose;
    fromMsg(transform.transform, frame);
    fromMsg(t_in, pose);
    toMsg(frame * pose, t_out);
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
