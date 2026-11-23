#pragma once

/// @file
/// tf2_ros::TransformBroadcaster: each sendTransform() publishes one tf2_msgs/TFMessage on
/// `/tf` with the `tf` policy (hypha::tfQos). A failed publish is logged and counted, as
/// an rclcpp::Publisher's.

#include <geometry_msgs/msg/transform_stamped.hpp>
#include <rclcpp/rclcpp.hpp>
#include <tf2_msgs/msg/tf_message.hpp>

#include <vector>

namespace tf2_ros {

class TransformBroadcaster
{
  public:
    explicit TransformBroadcaster(rclcpp::Node& node)
        : publisher_(node.hypha_node().advertise<tf2_msgs::TFMessage>(hypha::Topic{"/tf"}))
    {
    }

    void sendTransform(const geometry_msgs::msg::TransformStamped& transform)
    {
        this->sendTransform(std::vector<geometry_msgs::msg::TransformStamped>{transform});
    }

    void sendTransform(const std::vector<geometry_msgs::msg::TransformStamped>& transforms)
    {
        tf2_msgs::TFMessage message;
        message.transforms = transforms;
        if (!this->publisher_.publish(message)) {
            ++this->failed_;
            rclcpp::detail::reportPublishFailure(
                this->publisher_.topic(), this->publisher_.lastPublishStatus()
            );
        }
    }

    /// Shim only.
    uint64_t failed_publish_count() const
    {
        return this->failed_;
    }

  private:
    hypha::Publisher<tf2_msgs::TFMessage> publisher_;
    uint64_t                              failed_{0};
};

}  // namespace tf2_ros
