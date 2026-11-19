#pragma once

/// @file
/// The service policies of the MRS stack. Every type this file does not name has
/// hypha's `default` policy (8 publishers, 16 subscribers, depth 32).
///
/// plugin_host.h includes this file, so every program of the stack declares the same
/// policy for a type whichever end of the topic it holds.

#include <hypha/qos.h>

#include <mrs_msgs/UavState.h>

namespace mrs_host::policies {

/// The state estimate: sent at 100 Hz and read in full by the 10 Hz mission, whose
/// port examines 64 samples per step, twice the queue of the default policy.
inline constexpr hypha::Qos UAV_STATE{
    .name = "mrs_uav_state", .publishers = 4, .subscribers = 16, .maxDepth = 64
};

}  // namespace mrs_host::policies

template <>
struct hypha::QosOf<mrs_msgs::UavState>
{
    static constexpr hypha::Qos value = mrs_host::policies::UAV_STATE;
};
