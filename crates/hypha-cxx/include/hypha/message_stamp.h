#pragma once

/// @file
/// The checked conversions between hypha::Time and the RosTime wire type.

#include "hypha_base/time.h"
#include "hypha_msgs/ros_time.h"

#include <cstdint>
#include <limits>
#include <optional>
#include <stdexcept>
#include <string>

namespace hypha {

/// @brief Convert @p value to the RosTime wire type.
/// @return The wire value. The conversion never wraps.
/// @throws std::out_of_range when @p value is negative or its whole seconds do not
///         fit in 32 bits.
inline hypha_msgs::RosTime toRosTime(Time value)
{
    constexpr int64_t NANOS_PER_SECOND = 1'000'000'000;
    constexpr int64_t MAX_SECONDS = std::numeric_limits<uint32_t>::max();

    const int64_t nanos = value.nanos();
    const bool    outsideRange = nanos < 0 || nanos / NANOS_PER_SECOND > MAX_SECONDS;
    if (outsideRange) {
        throw std::out_of_range(
            "hypha: time " + std::to_string(nanos) + " ns does not fit a RosTime"
        );
    }
    const int64_t       seconds = nanos / NANOS_PER_SECOND;
    hypha_msgs::RosTime stamp{};
    stamp.secs = static_cast<uint32_t>(seconds);
    stamp.nsecs = static_cast<uint32_t>(nanos % NANOS_PER_SECOND);
    return stamp;
}

/// @brief toRosTime() of a time in seconds since the epoch, as a driver computes one:
///        `header.stamp = hypha::rosTimeFromSeconds(packetSeconds)`. A named function, not
///        an overload, so that seconds and nanoseconds are never mistaken for each other.
/// @throws std::invalid_argument for NaN or infinite seconds; std::out_of_range as
///         toRosTime() does.
inline hypha_msgs::RosTime rosTimeFromSeconds(double seconds)
{
    return toRosTime(Time::fromSeconds(seconds));
}

/// @brief toRosTime() of a time in nanoseconds since the epoch.
/// @throws std::out_of_range as toRosTime() does.
inline hypha_msgs::RosTime rosTimeFromNanos(int64_t nanos)
{
    return toRosTime(Time::fromNanos(nanos));
}

/// @brief Convert a RosTime wire stamp, such as a received header.stamp, to framework time.
/// @return No value when the nanosecond remainder is one second or more.
constexpr std::optional<Time> fromRosTime(const hypha_msgs::RosTime& stamp) noexcept
{
    constexpr int64_t NANOS_PER_SECOND = 1'000'000'000;
    if (stamp.nsecs >= static_cast<uint32_t>(NANOS_PER_SECOND)) {
        return std::nullopt;
    }
    const int64_t nanos =
        static_cast<int64_t>(stamp.secs) * NANOS_PER_SECOND + static_cast<int64_t>(stamp.nsecs);
    return Time::fromNanos(nanos);
}

}  // namespace hypha
