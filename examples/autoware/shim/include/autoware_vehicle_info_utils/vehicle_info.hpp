#pragma once

/// @file
/// autoware_vehicle_info_utils' VehicleInfo, the scalar part. The base parameters, the
/// derived ones and their checks are upstream's createVehicleInfo (autoware_core 1.10.0,
/// common/autoware_vehicle_info_utils/src/vehicle_info.cpp, Apache 2.0). The footprint
/// functions, which need Boost.Geometry and autoware_utils_geometry, are not offered: a
/// node that calls them does not compile.

#include <rclcpp/rclcpp.hpp>

#include <cmath>
#include <cstddef>

namespace autoware::vehicle_info_utils {

struct VehicleInfo
{
    double wheel_radius_m{};
    double wheel_width_m{};
    double wheel_base_m{};
    double wheel_tread_m{};
    double front_overhang_m{};
    double rear_overhang_m{};
    double left_overhang_m{};
    double right_overhang_m{};
    double vehicle_height_m{};
    double max_steer_angle_rad{};

    double vehicle_length_m{};
    double vehicle_width_m{};
    double min_longitudinal_offset_m{};
    double max_longitudinal_offset_m{};
    double min_lateral_offset_m{};
    double max_lateral_offset_m{};
    double min_height_offset_m{};
    double max_height_offset_m{};

    static constexpr size_t FrontLeftIndex = 0;
    static constexpr size_t FrontRightIndex = 1;
    static constexpr size_t RearRightIndex = 3;
    static constexpr size_t RearLeftIndex = 4;
};

/// Upstream's createVehicleInfo: clamps a zero wheel base and steering angle, reports
/// non-positive values, derives the extents.
inline VehicleInfo createVehicleInfo(
    double wheel_radius_m,
    double wheel_width_m,
    double wheel_base_m,
    double wheel_tread_m,
    double front_overhang_m,
    double rear_overhang_m,
    double left_overhang_m,
    double right_overhang_m,
    double vehicle_height_m,
    double max_steer_angle_rad
)
{
    constexpr double minWheelBase = 1e-6;
    if (std::abs(wheel_base_m) < minWheelBase) {
        RCLCPP_ERROR(
            rclcpp::get_logger("vehicle_info"),
            "wheel_base_m %f is almost 0.0, clamping to %f",
            wheel_base_m,
            minWheelBase
        );
        wheel_base_m = minWheelBase;
    }
    constexpr double minSteerAngle = 1e-6;
    if (std::abs(max_steer_angle_rad) < minSteerAngle) {
        RCLCPP_ERROR(
            rclcpp::get_logger("vehicle_info"),
            "max_steer_angle_rad %f is almost 0.0, clamping to %f",
            max_steer_angle_rad,
            minSteerAngle
        );
        max_steer_angle_rad = minSteerAngle;
    }
    const bool nonPositive =
        wheel_radius_m <= 0.0 || wheel_width_m <= 0.0 || wheel_base_m <= 0.0 || wheel_tread_m <= 0.0
        || front_overhang_m <= 0.0 || rear_overhang_m <= 0.0 || left_overhang_m <= 0.0
        || right_overhang_m <= 0.0 || vehicle_height_m <= 0.0 || max_steer_angle_rad <= 0.0;
    if (nonPositive) {
        RCLCPP_ERROR(
            rclcpp::get_logger("vehicle_info"), "given parameters contain non positive values"
        );
    }
    VehicleInfo info;
    info.wheel_radius_m = wheel_radius_m;
    info.wheel_width_m = wheel_width_m;
    info.wheel_base_m = wheel_base_m;
    info.wheel_tread_m = wheel_tread_m;
    info.front_overhang_m = front_overhang_m;
    info.rear_overhang_m = rear_overhang_m;
    info.left_overhang_m = left_overhang_m;
    info.right_overhang_m = right_overhang_m;
    info.vehicle_height_m = vehicle_height_m;
    info.max_steer_angle_rad = max_steer_angle_rad;
    info.vehicle_length_m = front_overhang_m + wheel_base_m + rear_overhang_m;
    info.vehicle_width_m = wheel_tread_m + left_overhang_m + right_overhang_m;
    info.min_longitudinal_offset_m = -rear_overhang_m;
    info.max_longitudinal_offset_m = front_overhang_m + wheel_base_m;
    info.min_lateral_offset_m = -(wheel_tread_m / 2.0 + right_overhang_m);
    info.max_lateral_offset_m = wheel_tread_m / 2.0 + left_overhang_m;
    info.min_height_offset_m = 0.0;
    info.max_height_offset_m = vehicle_height_m;
    return info;
}

}  // namespace autoware::vehicle_info_utils
