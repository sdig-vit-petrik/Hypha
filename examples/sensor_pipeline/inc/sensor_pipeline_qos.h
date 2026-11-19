#pragma once

/// @file The service policies of the sensor topics. The source, the processor and every
///       other node of the stack include this header, so either one may start first.
///       A tool declares none: start the application nodes before the tools.

#include <hypha/qos.h>
#include <hypha_msgs/sensor_msgs/Imu.h>
#include <hypha_msgs/sensor_msgs/PointCloud2.h>

#include <cstdint>

namespace sensor_pipeline {

/// The IMU: 400 Hz, and a queue of 200 samples, the `queue_size` of the ROS node. A
/// subscriber whose run() thread stalls loses samples after 200 / 400 Hz = 0.5 s. Two
/// publishers: the source, or `hypha play` in a replay.
inline constexpr hypha::Qos IMU_QOS{
    .name = "imu", .publishers = 2, .subscribers = 4, .maxDepth = 200, .initialPayload = 512
};

/// The largest point cloud a publisher sends, in serialized bytes: 16 rings x 1024 columns
/// x 16 bytes per point is 262144 bytes of data, plus the header and the field table.
inline constexpr uint64_t CLOUD_MAX_BYTES = 270'000;

/// The point clouds: 10 Hz, large, and a short queue. The pool of each publisher holds
/// a queue of this depth for every subscriber the policy admits, at CLOUD_MAX_BYTES a
/// sample, so the depth stays small.
inline constexpr hypha::Qos CLOUD_QOS{
    .name = "lidar_cloud",
    .publishers = 2,
    .subscribers = 4,
    .maxDepth = 4,
    .maxPayloadDynamic = CLOUD_MAX_BYTES
};

}  // namespace sensor_pipeline

template <>
struct hypha::QosOf<sensor_msgs::Imu>
{
    static constexpr hypha::Qos value = sensor_pipeline::IMU_QOS;
};

template <>
struct hypha::QosOf<sensor_msgs::PointCloud2>
{
    static constexpr hypha::Qos value = sensor_pipeline::CLOUD_QOS;
};
