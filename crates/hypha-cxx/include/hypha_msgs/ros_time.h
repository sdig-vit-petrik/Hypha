#pragma once

#include <cstdint>

/// RosTime/RosDuration: the `time`/`duration` fields of generated messages. Every
/// generated header includes this file. Layout matches crates/hypha_wire/src/ros_time.rs.
namespace hypha_msgs {

struct RosTime
{
    uint32_t secs{0};
    uint32_t nsecs{0};
};

struct RosDuration
{
    int32_t secs{0};
    int32_t nsecs{0};
};

}  // namespace hypha_msgs
