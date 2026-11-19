#pragma once

/// @file The service policy of `/large_frames`. Both programs include it, so either one
///       may start first: the first creates the topic with this policy and the other
///       must declare the same one.

#include <hypha/qos.h>

namespace hypha_bench {

/// One source, a few sinks, and a short queue: a frame is megabytes, and the pool of
/// the source holds a queue of this depth for every subscriber the policy admits.
inline constexpr hypha::Qos LARGE_FRAME_QOS{
    .name = "large_frames", .publishers = 1, .subscribers = 2, .maxDepth = 4
};

}  // namespace hypha_bench
