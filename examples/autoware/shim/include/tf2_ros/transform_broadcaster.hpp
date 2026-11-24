#pragma once

/// @file
/// tf2_ros::TransformBroadcaster over hypha::TransformBroadcaster: each sendTransform()
/// publishes one tf2_msgs/TFMessage on `/tf` with the `tf` policy (hypha::tfQos). A
/// transform every buffer would refuse (not finite, not a unit quaternion, a stamp that
/// is no time, an empty frame name) is not sent; that and a failed publish are logged and
/// counted, as an rclcpp::Publisher's failed publish.

#include <hypha/transform_buffer.h>

#include <geometry_msgs/msg/transform_stamped.hpp>
#include <rclcpp/rclcpp.hpp>
#include <tf2_msgs/msg/tf_message.hpp>

#include <cstdint>
#include <string>
#include <vector>

namespace tf2_ros {

class TransformBroadcaster
{
  public:
    explicit TransformBroadcaster(rclcpp::Node& node) : broadcaster_(node.hypha_node())
    {
    }

    void sendTransform(const geometry_msgs::msg::TransformStamped& transform)
    {
        this->sendTransform(std::vector<geometry_msgs::msg::TransformStamped>{transform});
    }

    void sendTransform(const std::vector<geometry_msgs::msg::TransformStamped>& transforms)
    {
        if (this->broadcaster_.send(transforms)) {
            return;
        }
        ++this->failed_;
        const std::string& refusal = this->broadcaster_.lastRefusal();
        if (!refusal.empty()) {
            HYPHA_LOG_ERROR_THROTTLE(1.0, "[tf2_ros] {}", refusal);
            return;
        }
        rclcpp::detail::reportPublishFailure(
            this->broadcaster_.topic(), this->broadcaster_.lastPublishStatus()
        );
    }

    /// Shim only: the sends refused or not published.
    uint64_t failed_publish_count() const
    {
        return this->failed_;
    }

  private:
    hypha::TransformBroadcaster broadcaster_;
    uint64_t                    failed_{0};
};

}  // namespace tf2_ros
