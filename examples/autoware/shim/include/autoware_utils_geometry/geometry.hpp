#pragma once

/// @file
/// The part of autoware_utils_geometry/geometry.hpp (autoware_utils 1.11.0) the port uses:
/// the distances between two points that have `x`, `y` and `z`. Upstream's header brings
/// the planning messages, tf2 and Boost.Geometry with it.

#include <cmath>

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

}  // namespace autoware_utils_geometry
