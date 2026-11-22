#pragma once

/// @file
/// managed_transform_buffer::ManagedTransformBuffer on hypha::TransformBuffer (`/tf`,
/// `/tf_static`), for the transform lookups of the concatenate-and-time-sync node.
///
/// A lookup never waits (hypha#91): the buffer fills only while the node runs, on the
/// run's thread, and the node looks up inside its callbacks, so the timeout is not used.
/// A static transform holds for all time, so a static calibration chain is found at any
/// stamp. A transform the buffer does not hold gives an empty result, as a lookup that
/// timed out does upstream, and a throttled warning that says why.

#include <hypha/hypha.h>
#include <hypha/transform_buffer.h>

#include <geometry_msgs/msg/transform_stamped.hpp>
#include <rclcpp/rclcpp.hpp>

#include <optional>
#include <string>
#include <string_view>
#include <type_traits>

namespace managed_transform_buffer {

namespace detail {
inline std::string_view reasonOf(hypha::TransformError error)
{
    switch (error) {
        case hypha::TransformError::InvalidFrame:
            return "a frame name is empty";
        case hypha::TransformError::UnknownFrame:
            return "a frame is unknown";
        case hypha::TransformError::NotConnected:
            return "the frames are not connected";
        case hypha::TransformError::BeforeBuffer:
            return "the stamp is older than the buffer";
        case hypha::TransformError::AfterBuffer:
            return "the stamp is newer than the buffer";
    }
    return "unknown";
}
}  // namespace detail

class ManagedTransformBuffer
{
  public:
    /// Subscribes `/tf` and `/tf_static` for the node of the process; create it in the
    /// node's constructor, before the node runs, as upstream does.
    ManagedTransformBuffer() : buffer_(this->session_)
    {
    }

    /// The transform from @p source_frame coordinates into @p target_frame at @p time.
    /// Empty when the buffer does not hold it.
    template <typename T = geometry_msgs::msg::TransformStamped>
    std::optional<T> getTransform(
        const std::string&  target_frame,
        const std::string&  source_frame,
        const rclcpp::Time& time,
        const rclcpp::Duration& /*timeout*/,
        const rclcpp::Logger& logger
    )
    {
        static_assert(
            std::is_same_v<T, geometry_msgs::msg::TransformStamped>,
            "managed_transform_buffer shim: only TransformStamped lookups are offered"
        );
        auto found = this->buffer_.lookup(target_frame, source_frame, time.toHypha());
        if (found.has_value()) {
            return std::move(*found);
        }
        const std::string reason(detail::reasonOf(found.error()));
        RCLCPP_WARN_THROTTLE(
            logger,
            nullptr,
            5000,
            "no transform from %s into %s: %s",
            source_frame.c_str(),
            target_frame.c_str(),
            reason.c_str()
        );
        return std::nullopt;
    }

  private:
    // A handle on the process session; declared first, so the buffer is built after it.
    hypha::Node            session_;
    hypha::TransformBuffer buffer_;
};

}  // namespace managed_transform_buffer
