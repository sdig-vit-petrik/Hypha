#pragma once

/// @file
/// tf2_ros::Buffer and tf2_ros::TransformListener on hypha::TransformBuffer (`/tf`,
/// `/tf_static`). The listener creates the hypha buffer on its node and hands it to the
/// Buffer; a lookup before that, or one that fails, throws the tf2 exception of its kind.
/// A lookup never waits: a timeout is refused (hypha#91, decision (b)).

#include <hypha/transform_buffer.h>

#include <geometry_msgs/msg/transform_stamped.hpp>
#include <rclcpp/rclcpp.hpp>
#include <tf2/exceptions.hpp>
#include <tf2/time.hpp>

#include <chrono>
#include <memory>
#include <string>

namespace tf2_ros {

class TransformListener;

class Buffer
{
  public:
    /// The clock is the framework clock, as every clock of the shim.
    explicit Buffer(const rclcpp::Clock::SharedPtr& /*clock*/)
    {
    }

    Buffer(const Buffer&) = delete;
    Buffer& operator=(const Buffer&) = delete;

    /// The transform from @p source coordinates into @p target at @p time;
    /// tf2::TimePointZero is the latest time the chain has in common.
    /// @throws tf2::LookupException (unknown or invalid frame), tf2::ConnectivityException,
    ///         tf2::ExtrapolationException (outside the buffer), tf2::TransformException
    ///         (no listener yet), std::invalid_argument for a timeout.
    geometry_msgs::msg::TransformStamped lookupTransform(
        const std::string&    target,
        const std::string&    source,
        const tf2::TimePoint& time,
        const tf2::Duration   timeout = tf2::Duration::zero()
    ) const
    {
        if (timeout != tf2::Duration::zero()) {
            throw std::invalid_argument(
                "tf2_ros::Buffer: a lookup never waits on hypha; give no timeout"
            );
        }
        if (!this->transforms_) {
            throw tf2::TransformException(
                "tf2_ros::Buffer: no TransformListener feeds this buffer yet"
            );
        }
        const auto found =
            time == tf2::TimePointZero
                ? this->transforms_->lookupLatest(target, source)
                : this->transforms_->lookup(
                      target, source, hypha::Time::fromNanos(time.time_since_epoch().count())
                  );
        if (found) {
            return *found;
        }
        const std::string what = "tf2_ros::Buffer: " + source + " to " + target + ": "
                               + std::string(hypha::name(found.error()));
        switch (found.error()) {
            case hypha::TransformError::InvalidFrame:
            case hypha::TransformError::UnknownFrame:
                throw tf2::LookupException(what);
            case hypha::TransformError::NotConnected:
                throw tf2::ConnectivityException(what);
            case hypha::TransformError::BeforeBuffer:
            case hypha::TransformError::AfterBuffer:
                throw tf2::ExtrapolationException(what);
        }
        throw tf2::TransformException(what);
    }

  private:
    friend class TransformListener;

    std::unique_ptr<hypha::TransformBuffer> transforms_;
};

/// Subscribes `/tf` and `/tf_static` on @p node for @p buffer. A spin thread is never
/// started: the transforms arrive on the node's spin thread.
class TransformListener
{
  public:
    TransformListener(Buffer& buffer, rclcpp::Node& node, bool /*spin_thread*/ = true)
    {
        if (buffer.transforms_) {
            throw std::invalid_argument("tf2_ros::TransformListener: the buffer has one already");
        }
        buffer.transforms_ = std::make_unique<hypha::TransformBuffer>(node.hypha_node());
    }

    TransformListener(const TransformListener&) = delete;
    TransformListener& operator=(const TransformListener&) = delete;
};

}  // namespace tf2_ros
