#pragma once

/// @file
/// The types that `Transforms` and `TransformBuffer` lookups share.

#include "hypha/hypha_ffi.h"
#include "hypha/message_stamp.h"
#include "hypha_base/time.h"
#include <hypha_msgs/geometry_msgs/TransformStamped.h>

#include <cstdint>
#include <expected>
#include <stdexcept>
#include <string>
#include <string_view>

namespace hypha {

/// @brief Why a lookup found no transform.
enum class TransformError {
    /// A frame name is empty or only `/`.
    InvalidFrame,
    /// No transform names one of the frames.
    UnknownFrame,
    /// Both frames are known, but no chain of transforms connects them.
    NotConnected,
    /// The time is older than a transform on the chain still holds.
    BeforeBuffer,
    /// The time is newer than the newest sample of a transform on the chain.
    AfterBuffer,
};

/// @brief The name of @p error, such as "UnknownFrame"; "Unknown" for any other value.
constexpr std::string_view name(TransformError error)
{
    switch (error) {
        case TransformError::InvalidFrame:
            return "InvalidFrame";
        case TransformError::UnknownFrame:
            return "UnknownFrame";
        case TransformError::NotConnected:
            return "NotConnected";
        case TransformError::BeforeBuffer:
            return "BeforeBuffer";
        case TransformError::AfterBuffer:
            return "AfterBuffer";
    }
    return "Unknown";
}

/// @brief The topics a TF reader subscribes: a Transforms port or a TransformBuffer.
struct TransformTopics
{
    std::string tf{"/tf"};
    std::string tfStatic{"/tf_static"};
};

namespace detail {

/// @brief The TransformError of a `HYPHA_TF_*` result other than `HYPHA_TF_FOUND`.
/// @throws std::runtime_error naming @p caller for a result this SDK does not know.
inline TransformError transformErrorOf(uint32_t result, std::string_view caller)
{
    switch (result) {
        case HYPHA_TF_INVALID_FRAME:
            return TransformError::InvalidFrame;
        case HYPHA_TF_UNKNOWN_FRAME:
            return TransformError::UnknownFrame;
        case HYPHA_TF_NOT_CONNECTED:
            return TransformError::NotConnected;
        case HYPHA_TF_BEFORE_BUFFER:
            return TransformError::BeforeBuffer;
        case HYPHA_TF_AFTER_BUFFER:
            return TransformError::AfterBuffer;
        default:
            throw std::runtime_error(
                "hypha: " + std::string(caller) + " unknown result " + std::to_string(result)
            );
    }
}

/// @brief The answer of a C ABI lookup: the transform of @p raw, or the error of @p result.
/// @throws std::runtime_error for an unknown @p result, naming @p caller;
///         std::out_of_range when the stamp does not fit a RosTime.
inline std::expected<geometry_msgs::TransformStamped, TransformError> transformAnswer(
    uint32_t              result,
    const HyphaTransform& raw,
    std::string_view      target,
    std::string_view      source,
    std::string_view      caller
)
{
    if (result != HYPHA_TF_FOUND) {
        return std::unexpected(transformErrorOf(result, caller));
    }
    geometry_msgs::TransformStamped found{};
    found.header.frame_id = std::string(target);
    found.child_frame_id = std::string(source);
    found.header.stamp = toRosTime(Time::fromNanos(raw.stamp_nanos));
    found.transform.translation.x = raw.translation[0];
    found.transform.translation.y = raw.translation[1];
    found.transform.translation.z = raw.translation[2];
    found.transform.rotation.x = raw.rotation_xyzw[0];
    found.transform.rotation.y = raw.rotation_xyzw[1];
    found.transform.rotation.z = raw.rotation_xyzw[2];
    found.transform.rotation.w = raw.rotation_xyzw[3];
    return found;
}

}  // namespace detail

}  // namespace hypha
