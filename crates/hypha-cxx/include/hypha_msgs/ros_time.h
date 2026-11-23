#pragma once

#include <cstdint>

/// RosTime/RosDuration: the `time`/`duration` fields of generated messages. Every
/// generated header includes this file. Layout matches crates/hypha_wire/src/ros_time.rs.
/// Both compare field by field, as every generated message does.
namespace hypha_msgs {

struct RosTime
{
    uint32_t secs{0};
    uint32_t nsecs{0};

    bool operator==(const RosTime&) const = default;
};

struct RosDuration
{
    int32_t secs{0};
    int32_t nsecs{0};

    bool operator==(const RosDuration&) const = default;
};

}  // namespace hypha_msgs
