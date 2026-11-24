#pragma once

// Representative ROS 1 messages for the codec tests. The golden byte arrays in
// ros1_codec_golden.h were produced from these builders, so a test that encodes a builder
// and compares against the golden array proves the wire bytes did not move.

#include <geometry_msgs/PoseWithCovariance.h>
#include <geometry_msgs/Twist.h>
#include <geometry_msgs/TwistStamped.h>
#include <geometry_msgs/TwistWithCovariance.h>
#include <nav_msgs/Odometry.h>
#include <sensor_msgs/Imu.h>
#include <sensor_msgs/JointState.h>
#include <std_msgs/Bool.h>
#include <std_msgs/Duration.h>
#include <std_msgs/Empty.h>
#include <std_msgs/Float64MultiArray.h>
#include <std_msgs/Header.h>

#include <array>
#include <cstdint>
#include <string>
#include <vector>

namespace ros1_fixtures {

inline std_msgs::Header header()
{
    std_msgs::Header value;
    value.seq = 7U;
    value.stamp.secs = 1700000000U;
    value.stamp.nsecs = 123456789U;
    value.frame_id = "base_link";
    return value;
}

/// A nested message with variable arrays, a nested array of messages, and strings.
inline std_msgs::Float64MultiArray float64MultiArray()
{
    std_msgs::Float64MultiArray   value;
    std_msgs::MultiArrayDimension rows;
    rows.label = "rows";
    rows.size = 2U;
    rows.stride = 6U;
    std_msgs::MultiArrayDimension cols;
    cols.label = "cols";
    cols.size = 3U;
    cols.stride = 3U;
    value.layout.dim = {rows, cols};
    value.layout.data_offset = 4U;
    value.data = {0.5, -1.25, 3.0, 1.0e-300, -0.0, 2.5e8};
    return value;
}

/// Fixed-size POD arrays (float64[36] covariances) inside nested messages.
inline nav_msgs::Odometry odometry()
{
    nav_msgs::Odometry value;
    value.header = header();
    value.child_frame_id = "base_link";
    value.pose.pose.position.x = 1.5;
    value.pose.pose.position.y = -2.25;
    value.pose.pose.position.z = 0.125;
    value.pose.pose.orientation.x = 0.0;
    value.pose.pose.orientation.y = 0.0;
    value.pose.pose.orientation.z = 0.7071067811865476;
    value.pose.pose.orientation.w = 0.7071067811865476;
    for (size_t i = 0; i < value.pose.covariance.size(); ++i) {
        value.pose.covariance[i] = static_cast<double>(i) * 0.5 - 3.25;
    }
    value.twist.twist.linear.x = 0.75;
    value.twist.twist.angular.z = -0.2;
    for (size_t i = 0; i < value.twist.covariance.size(); ++i) {
        value.twist.covariance[i] = 1.0 / static_cast<double>(i + 1U);
    }
    return value;
}

/// Variable arrays of strings and of doubles, with an empty string and empty arrays.
inline sensor_msgs::JointState jointState()
{
    sensor_msgs::JointState value;
    value.header = header();
    value.name = {"joint_a", "", "joint_ccc"};
    value.position = {0.1, 0.2, 0.3};
    value.velocity = {};
    value.effort = {-9.5, 4.0, 0.0};
    return value;
}

/// Three fixed-size float64[9] arrays, the widest fixed arrays in the standard set.
inline sensor_msgs::Imu imu()
{
    sensor_msgs::Imu value;
    value.header = header();
    value.orientation.w = 1.0;
    for (size_t i = 0; i < 9U; ++i) {
        value.orientation_covariance[i] = static_cast<double>(i) - 4.0;
        value.angular_velocity_covariance[i] = 0.001 * static_cast<double>(i);
        value.linear_acceleration_covariance[i] = -0.5 * static_cast<double>(i);
    }
    value.angular_velocity.z = 0.03125;
    value.linear_acceleration.z = 9.81;
    return value;
}

/// A bool: one wire byte.
inline std_msgs::Bool boolTrue()
{
    std_msgs::Bool value;
    value.data = true;
    return value;
}

/// A ROS duration: two signed 32-bit fields.
inline std_msgs::Duration duration()
{
    std_msgs::Duration value;
    value.data.secs = -5;
    value.data.nsecs = 999999999;
    return value;
}

/// A message with no fields: zero bytes on the wire.
inline std_msgs::Empty empty()
{
    return std_msgs::Empty{};
}

inline geometry_msgs::TwistStamped twistStamped()
{
    geometry_msgs::TwistStamped value;
    value.header = header();
    value.twist.linear.x = -0.5;
    value.twist.angular.y = 2.0;
    return value;
}

}  // namespace ros1_fixtures
