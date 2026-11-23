#pragma once

/// @file
/// autoware_utils_debug::DebugPublisher on the shim: each name is an optional output
/// port `<ns>.<name with dots>` (`crop_box_filter.debug.processing_time_ms`). A
/// deployment wires the ones it reads; the rest are disabled and publish nothing.

#include <autoware_internal_debug_msgs/msg/float64_stamped.hpp>
#include <rclcpp/rclcpp.hpp>
#include <ros/message_traits.h>

#include <any>
#include <string>
#include <unordered_map>

namespace autoware_utils_debug {

class DebugPublisher
{
  public:
    DebugPublisher(rclcpp::Node* node, const char* ns) : node_(node), ns_(ns)
    {
    }

    /// Publishes a message under @p name.
    template <class T>
        requires ros::message_traits::IsMessage<T>::value
    void publish(const std::string& name, const T& data, const rclcpp::QoS& qos = rclcpp::QoS(1))
    {
        using PubPtr = typename rclcpp::Publisher<T>::SharedPtr;
        if (!this->pubs_.contains(name)) {
            rclcpp::PublisherOptions options;
            options.hypha_optional_topic = true;
            this->pubs_[name] =
                this->node_->create_publisher<T>(std::string(this->ns_) + "/" + name, qos, options);
        }
        std::any_cast<PubPtr&>(this->pubs_.at(name))->publish(data);
    }

    /// Publishes @p data as the `data` of a stamped debug message, stamped now.
    template <class TMsg, class T>
        requires(!ros::message_traits::IsMessage<T>::value)
    void publish(const std::string& name, const T& data, const rclcpp::QoS& qos = rclcpp::QoS(1))
    {
        TMsg msg;
        msg.stamp = this->node_->now();
        msg.data = data;
        this->publish(name, msg, qos);
    }

  private:
    rclcpp::Node*                             node_;
    const char*                               ns_;
    std::unordered_map<std::string, std::any> pubs_;
};

/// Upstream's template over the node type, of which DebugPublisher is the rclcpp::Node form. The
/// shim's only node is rclcpp::Node, which autoware::agnocast_wrapper::Node names too.
template <class NodeT>
using BasicDebugPublisher = DebugPublisher;

}  // namespace autoware_utils_debug
