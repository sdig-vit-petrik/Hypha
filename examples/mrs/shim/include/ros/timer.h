#pragma once

/// @file
/// ros::TimerEvent, the argument of timer callbacks. ros::Timer is in ros/node_handle.h.

#include <ros/time.h>

namespace ros {

struct TimerEvent
{
    Time last_expected;
    Time last_real;
    Time current_expected;
    Time current_real;

    struct
    {
        Duration last_duration;
    } profile;
};

}  // namespace ros
