#pragma once

/// @file
/// autoware_utils_debug::PublishedTimePublisher on the shim: for each publisher, a
/// `PublishedTime` on its topic plus `/debug/published_time` (a literal topic), sent
/// only while it has a subscriber.

#include <autoware_internal_msgs/msg/published_time.hpp>
#include <rclcpp/rclcpp.hpp>
#include <std_msgs/msg/header.hpp>

#include <map>
#include <string>
#include <utility>

namespace autoware_utils_debug {

using PublishedTime = autoware_internal_msgs::msg::PublishedTime;

class PublishedTimePublisher
{
  public:
    explicit PublishedTimePublisher(
        rclcpp::Node*      node,
        std::string        suffix = "/debug/published_time",
        const rclcpp::QoS& qos = rclcpp::QoS(1)
    )
        : node_(node), suffix_(std::move(suffix)), qos_(qos)
    {
    }

    template <class PubT>
    void publish_if_subscribed(const PubT& publisher, const rclcpp::Time& stamp)
    {
        std_msgs::msg::Header header;
        header.stamp = stamp;
        this->publish_if_subscribed(publisher, header);
    }

    template <class PubT>
    void publish_if_subscribed(const PubT& publisher, const std_msgs::msg::Header& header)
    {
        const auto& pub = this->publisherFor(publisher.get(), publisher->get_topic_name());
        if (pub->get_subscription_count() > 0) {
            PublishedTime published;
            published.header = header;
            published.published_stamp = this->node_->now();
            pub->publish(published);
        }
    }

  private:
    using PubPtr = rclcpp::Publisher<PublishedTime>::SharedPtr;

    const PubPtr& publisherFor(const void* key, const std::string& topic)
    {
        auto found = this->pubs_.find(key);
        if (found == this->pubs_.end()) {
            found = this->pubs_
                        .emplace(
                            key,
                            this->node_->create_publisher<PublishedTime>(
                                topic + this->suffix_, this->qos_
                            )
                        )
                        .first;
        }
        return found->second;
    }

    rclcpp::Node*                 node_;
    std::string                   suffix_;
    rclcpp::QoS                   qos_;
    std::map<const void*, PubPtr> pubs_;
};

}  // namespace autoware_utils_debug
