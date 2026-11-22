#pragma once

/// @file
/// autoware_utils_tf::TransformListener on hypha::TransformBuffer (`/tf`, `/tf_static`).
///
/// hypha ingests transforms only while the node runs. Before spin(), where Autoware nodes
/// look their static transforms up, get_transform() with a timeout runs the process's
/// callbacks in 10 ms slices until the transform is found or the timeout (host time)
/// has passed.

#include <rclcpp/rclcpp.hpp>

#include <geometry_msgs/msg/transform_stamped.hpp>
#include <hypha/transform_buffer.h>

#include <chrono>
#include <memory>
#include <string>

namespace autoware_utils_tf {

class TransformListener
{
  public:
    explicit TransformListener(rclcpp::Node* node)
        : node_(node), buffer_(std::make_unique<hypha::TransformBuffer>(node->hypha_node()))
    {
    }

    /// The transform from @p to coordinates into @p from, latest common time; empty when
    /// none is known.
    std::shared_ptr<const geometry_msgs::msg::TransformStamped>
    get_latest_transform(const std::string& from, const std::string& to)
    {
        const auto found = this->buffer_->lookupLatest(from, to);
        if (!found) {
            RCLCPP_WARN_THROTTLE(
                this->node_->get_logger(), *this->node_->get_clock(), 5000,
                "failed to get transform from %s to %s", from.c_str(), to.c_str()
            );
            return {};
        }
        return std::make_shared<const geometry_msgs::msg::TransformStamped>(*found);
    }

    /// The transform from @p to coordinates into @p from at @p time; waits up to
    /// @p timeout (see the file comment). Empty when none is found.
    std::shared_ptr<const geometry_msgs::msg::TransformStamped> get_transform(
        const std::string& from, const std::string& to, const rclcpp::Time& time,
        const rclcpp::Duration& timeout
    )
    {
        const auto deadline = std::chrono::steady_clock::now()
                              + std::chrono::nanoseconds(timeout.nanoseconds());
        for (;;) {
            const auto found = this->buffer_->lookup(from, to, time.toHypha());
            if (found) {
                return std::make_shared<const geometry_msgs::msg::TransformStamped>(*found);
            }
            const auto latest = this->buffer_->lookupLatest(from, to);
            const bool staticChain = latest.has_value() && latest->header.stamp.secs == 0
                                     && latest->header.stamp.nsecs == 0;
            if (staticChain) {
                return std::make_shared<const geometry_msgs::msg::TransformStamped>(*latest);
            }
            if (std::chrono::steady_clock::now() >= deadline) {
                RCLCPP_WARN_THROTTLE(
                    this->node_->get_logger(), *this->node_->get_clock(), 5000,
                    "failed to get transform from %s to %s", from.c_str(), to.c_str()
                );
                return {};
            }
            const hypha::RunResult ran =
                this->node_->hypha_node().runFor(hypha::Duration::fromMillis(10));
            if (!ran.ok()) {
                return {};
            }
        }
    }

  private:
    rclcpp::Node*                           node_;
    std::unique_ptr<hypha::TransformBuffer> buffer_;
};

}  // namespace autoware_utils_tf
