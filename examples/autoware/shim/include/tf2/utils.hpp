#pragma once

/// @file
/// tf2/utils.hpp on the vendored tf2 LinearMath (upstream/geometry2): getYaw and
/// getEulerYPR of a tf2 or a message quaternion. The formulas are tf2's own
/// (tf2/impl/utils.hpp, geometry2 jazzy f702874, BSD 3-Clause); upstream's header needs
/// tf2_geometry_msgs and KDL for the message conversion this one does in place.

#include <geometry_msgs/msg/quaternion.hpp>
#include <tf2/LinearMath/Quaternion.hpp>

#include <cmath>

namespace tf2 {

namespace impl {

inline tf2::Quaternion toQuaternion(const tf2::Quaternion& q)
{
    return q;
}

inline tf2::Quaternion toQuaternion(const geometry_msgs::msg::Quaternion& q)
{
    return tf2::Quaternion(q.x, q.y, q.z, q.w);
}

/// tf2's: yaw, pitch and roll of @p q. At a singularity of pitch, tf2 gives a pitch of
/// ±0.5 * pi/2, not ±pi/2; kept, so the port computes what ROS computes.
inline void getEulerYPR(const tf2::Quaternion& q, double& yaw, double& pitch, double& roll)
{
    const double pi_2 = 1.57079632679489661923;
    const double sqw = q.w() * q.w();
    const double sqx = q.x() * q.x();
    const double sqy = q.y() * q.y();
    const double sqz = q.z() * q.z();
    const double sarg = -2 * (q.x() * q.z() - q.w() * q.y()) / (sqx + sqy + sqz + sqw);
    if (sarg <= -0.99999) {
        pitch = -0.5 * pi_2;
        roll = 0;
        yaw = -2 * std::atan2(q.y(), q.x());
    } else if (sarg >= 0.99999) {
        pitch = 0.5 * pi_2;
        roll = 0;
        yaw = 2 * std::atan2(q.y(), q.x());
    } else {
        pitch = std::asin(sarg);
        roll = std::atan2(2 * (q.y() * q.z() + q.w() * q.x()), sqw - sqx - sqy + sqz);
        yaw = std::atan2(2 * (q.x() * q.y() + q.w() * q.z()), sqw + sqx - sqy - sqz);
    }
}

/// tf2's: the yaw of @p q.
inline double getYaw(const tf2::Quaternion& q)
{
    const double sqw = q.w() * q.w();
    const double sqx = q.x() * q.x();
    const double sqy = q.y() * q.y();
    const double sqz = q.z() * q.z();
    const double sarg = -2 * (q.x() * q.z() - q.w() * q.y()) / (sqx + sqy + sqz + sqw);
    if (sarg <= -0.99999) {
        return -2 * std::atan2(q.y(), q.x());
    }
    if (sarg >= 0.99999) {
        return 2 * std::atan2(q.y(), q.x());
    }
    return std::atan2(2 * (q.x() * q.y() + q.w() * q.z()), sqw + sqx - sqy - sqz);
}

}  // namespace impl

template <class A>
void getEulerYPR(const A& a, double& yaw, double& pitch, double& roll)
{
    impl::getEulerYPR(impl::toQuaternion(a), yaw, pitch, roll);
}

template <class A>
double getYaw(const A& a)
{
    return impl::getYaw(impl::toQuaternion(a));
}

}  // namespace tf2
