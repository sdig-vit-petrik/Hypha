#pragma once
/// The part of tf2_geometry_msgs (geometry2 noetic) the vendored MRS code uses: the same
/// includes, and the fromMsg/toMsg overloads between geometry_msgs and tf2's LinearMath.
/// The noetic header also needs KDL and tf2/convert.h; neither is vendored.
#include <geometry_msgs/Point.h>
#include <geometry_msgs/PointStamped.h>
#include <geometry_msgs/Pose.h>
#include <geometry_msgs/PoseStamped.h>
#include <geometry_msgs/Quaternion.h>
#include <geometry_msgs/QuaternionStamped.h>
#include <geometry_msgs/Transform.h>
#include <geometry_msgs/TransformStamped.h>
#include <geometry_msgs/Vector3.h>
#include <geometry_msgs/Vector3Stamped.h>
#include <tf2/LinearMath/Quaternion.h>
#include <tf2/LinearMath/Transform.h>
#include <tf2/LinearMath/Vector3.h>

namespace tf2 {

inline geometry_msgs::Vector3 toMsg(const tf2::Vector3& in)
{
    geometry_msgs::Vector3 out;
    out.x = in.getX();
    out.y = in.getY();
    out.z = in.getZ();
    return out;
}

inline void fromMsg(const geometry_msgs::Vector3& in, tf2::Vector3& out)
{
    out = tf2::Vector3(in.x, in.y, in.z);
}

inline geometry_msgs::Point& toMsg(const tf2::Vector3& in, geometry_msgs::Point& out)
{
    out.x = in.getX();
    out.y = in.getY();
    out.z = in.getZ();
    return out;
}

inline void fromMsg(const geometry_msgs::Point& in, tf2::Vector3& out)
{
    out = tf2::Vector3(in.x, in.y, in.z);
}

inline geometry_msgs::Quaternion toMsg(const tf2::Quaternion& in)
{
    geometry_msgs::Quaternion out;
    out.w = in.getW();
    out.x = in.getX();
    out.y = in.getY();
    out.z = in.getZ();
    return out;
}

inline void fromMsg(const geometry_msgs::Quaternion& in, tf2::Quaternion& out)
{
    out = tf2::Quaternion(in.x, in.y, in.z, in.w);
}

}  // namespace tf2
