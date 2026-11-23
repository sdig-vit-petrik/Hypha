#pragma once

/// @file
/// The part of autoware_utils_geometry/geometry.hpp (autoware_utils 1.11.0) the port uses,
/// as upstream writes it (src/geometry/geometry.cpp for the functions it defines there):
/// the distances between two points that have `x`, `y` and `z`, the roll, pitch and yaw of
/// a quaternion, a quaternion from them, a point, a translation, a pose offset in its own
/// frame (calc_offset_pose), and the transform of a pose. Upstream's
/// header brings the planning messages and Boost.Geometry with it; the tf2 types are the
/// vendored ones (upstream/geometry2). `msg/covariance.hpp` is upstream's, unchanged.

#include <autoware_utils_geometry/msg/covariance.hpp>
#include <geometry_msgs/msg/point.hpp>
#include <geometry_msgs/msg/pose.hpp>
#include <geometry_msgs/msg/pose_stamped.hpp>
#include <geometry_msgs/msg/pose_with_covariance_stamped.hpp>
#include <geometry_msgs/msg/quaternion.hpp>
#include <geometry_msgs/msg/transform.hpp>
#include <geometry_msgs/msg/transform_stamped.hpp>
#include <geometry_msgs/msg/vector3.hpp>
#include <tf2/LinearMath/Matrix3x3.hpp>
#include <tf2/LinearMath/Quaternion.hpp>
#include <tf2_geometry_msgs/tf2_geometry_msgs.hpp>

#include <cmath>
#include <string>

namespace autoware_utils_geometry {

template <class Point1, class Point2>
double calc_distance2d(const Point1& point1, const Point2& point2)
{
    return std::hypot(
        static_cast<double>(point1.x) - static_cast<double>(point2.x),
        static_cast<double>(point1.y) - static_cast<double>(point2.y)
    );
}

template <class Point1, class Point2>
double calc_squared_distance2d(const Point1& point1, const Point2& point2)
{
    const double dx = static_cast<double>(point1.x) - static_cast<double>(point2.x);
    const double dy = static_cast<double>(point1.y) - static_cast<double>(point2.y);
    return dx * dx + dy * dy;
}

template <class Point1, class Point2>
double calc_distance3d(const Point1& point1, const Point2& point2)
{
    return std::hypot(
        calc_distance2d(point1, point2),
        static_cast<double>(point1.z) - static_cast<double>(point2.z)
    );
}

inline geometry_msgs::msg::Point create_point(const double x, const double y, const double z)
{
    geometry_msgs::msg::Point p;
    p.x = x;
    p.y = y;
    p.z = z;
    return p;
}

inline geometry_msgs::msg::Vector3 get_rpy(const geometry_msgs::msg::Quaternion& quat)
{
    geometry_msgs::msg::Vector3 rpy;
    tf2::Quaternion             q(quat.x, quat.y, quat.z, quat.w);
    tf2::Matrix3x3(q).getRPY(rpy.x, rpy.y, rpy.z);
    return rpy;
}

inline geometry_msgs::msg::Vector3 get_rpy(const geometry_msgs::msg::Pose& pose)
{
    return get_rpy(pose.orientation);
}

inline geometry_msgs::msg::Vector3 get_rpy(const geometry_msgs::msg::PoseStamped& pose)
{
    return get_rpy(pose.pose);
}

inline geometry_msgs::msg::Vector3
get_rpy(const geometry_msgs::msg::PoseWithCovarianceStamped& pose)
{
    return get_rpy(pose.pose.pose);
}

inline geometry_msgs::msg::Quaternion
create_quaternion_from_rpy(const double roll, const double pitch, const double yaw)
{
    tf2::Quaternion q;
    q.setRPY(roll, pitch, yaw);
    return tf2::toMsg(q);
}

inline geometry_msgs::msg::Quaternion create_quaternion_from_yaw(const double yaw)
{
    tf2::Quaternion q;
    q.setRPY(0, 0, yaw);
    return tf2::toMsg(q);
}

inline geometry_msgs::msg::Vector3
create_translation(const double x, const double y, const double z)
{
    geometry_msgs::msg::Vector3 v;
    v.x = x;
    v.y = y;
    v.z = z;
    return v;
}

/// @p p moved by (x, y, z) and turned by @p yaw in its own frame.
inline geometry_msgs::msg::Pose calc_offset_pose(
    const geometry_msgs::msg::Pose& p,
    const double                    x,
    const double                    y,
    const double                    z,
    const double                    yaw = 0.0
)
{
    geometry_msgs::msg::Pose      pose;
    geometry_msgs::msg::Transform transform;
    transform.translation = create_translation(x, y, z);
    transform.rotation = create_quaternion_from_yaw(yaw);
    tf2::Transform tf_pose;
    tf2::Transform tf_offset;
    tf2::fromMsg(transform, tf_offset);
    tf2::fromMsg(p, tf_pose);
    tf2::toMsg(tf_pose * tf_offset, pose);
    return pose;
}

inline geometry_msgs::msg::Transform pose2transform(const geometry_msgs::msg::Pose& pose)
{
    geometry_msgs::msg::Transform transform;
    transform.translation.x = pose.position.x;
    transform.translation.y = pose.position.y;
    transform.translation.z = pose.position.z;
    transform.rotation = pose.orientation;
    return transform;
}

inline geometry_msgs::msg::TransformStamped
pose2transform(const geometry_msgs::msg::PoseStamped& pose, const std::string& child_frame_id)
{
    geometry_msgs::msg::TransformStamped transform;
    transform.header = pose.header;
    transform.transform = pose2transform(pose.pose);
    transform.child_frame_id = child_frame_id;
    return transform;
}

}  // namespace autoware_utils_geometry
