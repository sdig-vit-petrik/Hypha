#pragma once

/// @file
/// The tf2_eigen conversions the port uses, on hypha's geometry_msgs.

#include <hypha_msgs/geometry_msgs/Transform.h>
#include <hypha_msgs/geometry_msgs/TransformStamped.h>

#include <Eigen/Geometry>

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

}  // namespace tf2
