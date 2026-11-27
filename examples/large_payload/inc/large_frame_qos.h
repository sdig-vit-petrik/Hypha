#pragma once

/// @file The identity and service policy of `/large_frames`. Both programs include it,
///       so either one may start first: the first creates the stream with this policy and
///       the other must declare the same one.

#include <hypha/qos.h>
#include <hypha/raw_api.h>

namespace hypha_bench {

/// A raw identity needs a data type and a 32-digit lower-case hex md5; the md5 names the
/// stream of the topic.
inline const hypha::RawType LARGE_FRAME_TYPE{
    .dataType = "hypha/LargeFrame", .md5 = "0123456789abcdef0123456789abcdef", .definition = ""
};

/// One source, a few sinks, and a short queue: a frame is megabytes, and the pool of
/// the source holds a queue of this depth for every subscriber the policy admits.
inline constexpr hypha::Qos LARGE_FRAME_QOS{
    .name = "large_frames", .publishers = 1, .subscribers = 2, .maxDepth = 4
};

}  // namespace hypha_bench
