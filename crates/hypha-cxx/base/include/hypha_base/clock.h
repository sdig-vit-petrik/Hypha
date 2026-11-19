#pragma once

#include "hypha/hypha_ffi.h"
#include "hypha_base/time.h"

namespace hypha {

/// @brief Framework time in UTC since the Unix epoch: simulated while simulated time is
///        enabled, else UTC wall time. Inside a cycle step it returns CycleTime::now.
inline Time now()
{
    return Time::fromNanos(hypha_clock_now_nanos());
}

/// @brief True while simulated time is enabled (HYPHA_USE_SIM_TIME).
inline bool isSimTime()
{
    return hypha_clock_is_sim_time() != 0;
}

/// @brief Real elapsed time (CLOCK_MONOTONIC), never simulated; it agrees with every
///        other process on the same host.
inline Time steadyNow()
{
    return Time::fromNanos(hypha_clock_steady_now_nanos());
}

/// @brief Start simulated framework time reading @p initial. Simulated time
///        that is already enabled is replaced.
/// @return false, and no change, when @p initial is negative.
[[nodiscard]] inline bool enableSimTime(Time initial)
{
    return hypha_clock_enable_sim_time(initial.nanos()) == HYPHA_STATUS_OK;
}

/// @brief Return framework time to the UTC wall clock.
/// @return false when the library refuses the change.
[[nodiscard]] inline bool disableSimTime()
{
    return hypha_clock_disable_sim_time() == HYPHA_STATUS_OK;
}

/// @brief Set simulated framework time to @p target, forward or backward.
/// @return false, and no change, while simulated time is disabled or when
///         @p target is negative.
[[nodiscard]] inline bool setSimTime(Time target)
{
    return hypha_clock_set_sim_time(target.nanos()) == HYPHA_STATUS_OK;
}

/// @brief Move simulated framework time forward by @p by.
/// @return false, and no change, while simulated time is disabled or when
///         @p by is negative.
[[nodiscard]] inline bool advanceSimTime(Duration by)
{
    return hypha_clock_advance_sim_time(by.nanos()) == HYPHA_STATUS_OK;
}

}  // namespace hypha
