#pragma once

/// @file
/// The tf2_eigen conversions the port uses, on hypha's geometry_msgs.

#include <Eigen/Geometry>
#include <geometry_msgs/Pose.h>
#include <geometry_msgs/Transform.h>
#include <geometry_msgs/TransformStamped.h>

namespace tf2 {

/// The isometry of @p t: its translation after its rotation, as tf2_eigen gives it.
inline Eigen::Isometry3d transformToEigen(const geometry_msgs::Transform& t)
{
    return Eigen::Isometry3d(
        Eigen::Translation3d(t.translation.x, t.translation.y, t.translation.z)
        * Eigen::Quaterniond(t.rotation.w, t.rotation.x, t.rotation.y, t.rotation.z)
    );
}

inline Eigen::Isometry3d transformToEigen(const geometry_msgs::TransformStamped& t)
{
    return transformToEigen(t.transform);
}

/// The pose @p msg as an affine transform, its quaternion normalized, as tf2_eigen does.
inline void fromMsg(const geometry_msgs::Pose& msg, Eigen::Affine3d& out)
{
    Eigen::Quaterniond quat(
        msg.orientation.w, msg.orientation.x, msg.orientation.y, msg.orientation.z
    );
    quat.normalize();
    out = Eigen::Affine3d(
        Eigen::Translation3d(msg.position.x, msg.position.y, msg.position.z) * quat
    );
}

/// As above, as an isometry.
inline void fromMsg(const geometry_msgs::Pose& msg, Eigen::Isometry3d& out)
{
    Eigen::Quaterniond quat(
        msg.orientation.w, msg.orientation.x, msg.orientation.y, msg.orientation.z
    );
    quat.normalize();
    out = Eigen::Isometry3d(
        Eigen::Translation3d(msg.position.x, msg.position.y, msg.position.z) * quat
    );
}

}  // namespace tf2
