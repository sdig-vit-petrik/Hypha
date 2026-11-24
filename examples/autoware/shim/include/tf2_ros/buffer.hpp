#pragma once

/// @file
/// tf2::BufferCore, tf2_ros::Buffer and tf2_ros::TransformListener on hypha::TransformBuffer
/// (`/tf`, `/tf_static`). As in ROS, the Buffer is a BufferCore with a clock, and the
/// listener feeds a BufferCore: it creates the hypha buffer on its node and hands it over.
/// A lookup before that, or one that fails, throws the tf2 exception of its kind.
/// A lookup never waits (hypha#91, decision (b)): a timeout is accepted and not waited
/// for, as the shim's managed_transform_buffer ignores it.

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
}  // namespace tf2_ros

namespace tf2 {

class BufferCore
{
  public:
    BufferCore() = default;

    BufferCore(const BufferCore&) = delete;
    BufferCore& operator=(const BufferCore&) = delete;

    /// The transform from @p source coordinates into @p target at @p time;
    /// tf2::TimePointZero is the latest time the chain has in common.
    /// @throws tf2::LookupException (unknown or invalid frame), tf2::ConnectivityException,
    ///         tf2::ExtrapolationException (outside the buffer), tf2::TransformException
    ///         (no listener yet).
    geometry_msgs::msg::TransformStamped lookupTransform(
        const std::string&    target,
        const std::string&    source,
        const tf2::TimePoint& time,
        const tf2::Duration /*timeout*/ = tf2::Duration::zero()
    ) const
    {
        if (!this->transforms_) {
            throw tf2::TransformException("tf2: no TransformListener feeds this buffer yet");
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
        const std::string what =
            "tf2: " + source + " to " + target + ": " + std::string(hypha::name(found.error()));
        switch (found.error()) {
            case hypha::TransformError::InvalidFrame:
            case hypha::TransformError::UnknownFrame:
                throw tf2::LookupException(what);
            case hypha::TransformError::NotConnected:
                throw tf2::ConnectivityException(what);
            case hypha::TransformError::BeforeBuffer:
            case hypha::TransformError::AfterBuffer:
                throw tf2::ExtrapolationException(what);
            case hypha::TransformError::Refused:
                // The producer sent an invalid transform: not a lookup to retry later.
                throw tf2::InvalidArgumentException(what);
        }
        throw tf2::TransformException(what);
    }

    /// As above, at an rclcpp time; tf2_ros::Buffer's overload.
    geometry_msgs::msg::TransformStamped lookupTransform(
        const std::string&  target,
        const std::string&  source,
        const rclcpp::Time& time,
        const rclcpp::Duration& /*timeout*/ = rclcpp::Duration::from_nanoseconds(0)
    ) const
    {
        return this->lookupTransform(
            target, source, tf2::TimePoint(tf2::Duration(time.nanoseconds()))
        );
    }

    /// Whether lookupTransform would find the transform; the reason in @p error_msg when not.
    bool canTransform(
        const std::string&    target,
        const std::string&    source,
        const tf2::TimePoint& time,
        const tf2::Duration   timeout = tf2::Duration::zero(),
        std::string*          error_msg = nullptr
    ) const
    {
        try {
            (void)this->lookupTransform(target, source, time, timeout);
            return true;
        } catch (const tf2::TransformException& failure) {
            if (error_msg != nullptr) {
                *error_msg = failure.what();
            }
            return false;
        }
    }

    bool canTransform(
        const std::string&  target,
        const std::string&  source,
        const rclcpp::Time& time,
        const rclcpp::Duration& /*timeout*/ = rclcpp::Duration::from_nanoseconds(0),
        std::string* error_msg = nullptr
    ) const
    {
        return this->canTransform(
            target,
            source,
            tf2::TimePoint(tf2::Duration(time.nanoseconds())),
            tf2::Duration::zero(),
            error_msg
        );
    }

  private:
    friend class tf2_ros::TransformListener;

    std::unique_ptr<hypha::TransformBuffer> transforms_;
};

}  // namespace tf2

namespace tf2_ros {

/// A BufferCore with a clock; the clock is the framework clock, as every clock of the shim.
class Buffer : public tf2::BufferCore
{
  public:
    explicit Buffer(const rclcpp::Clock::SharedPtr& /*clock*/)
    {
    }
};

/// Subscribes `/tf` and `/tf_static` for @p buffer. A spin thread is never started: the
/// transforms arrive on the spin thread of the process, which runs every subscription of
/// its session. Without a node, as ROS's standalone listener, it subscribes through a
/// handle on the process session.
class TransformListener
{
  public:
    explicit TransformListener(tf2::BufferCore& buffer, bool /*spin_thread*/ = true)
    {
        this->attach(buffer, this->session_);
    }

    TransformListener(tf2::BufferCore& buffer, rclcpp::Node& node, bool /*spin_thread*/ = true)
    {
        this->attach(buffer, node.hypha_node());
    }

    TransformListener(const TransformListener&) = delete;
    TransformListener& operator=(const TransformListener&) = delete;

  private:
    static void attach(tf2::BufferCore& buffer, hypha::Node& node)
    {
        if (buffer.transforms_) {
            throw std::invalid_argument("tf2_ros::TransformListener: the buffer has one already");
        }
        buffer.transforms_ = std::make_unique<hypha::TransformBuffer>(node);
    }

    /// A handle on the process session, for a listener without a node.
    hypha::Node session_;
};

}  // namespace tf2_ros
