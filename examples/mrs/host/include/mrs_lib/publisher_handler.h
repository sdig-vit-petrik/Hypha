#pragma once

/// @file
/// mrs_lib::PublisherHandler over an Output port of the host's cycle (hypha port of
/// the upstream header). publish() sends in the running step; outside a step it throws.

#include <ros/node_handle.h>

#include <memory>
#include <stdexcept>
#include <string>

namespace mrs_lib {

template <class TopicType>
class PublisherHandler
{
  public:
    PublisherHandler() = default;

    /// @p rate (a publishing rate limit upstream) is not supported and must be 0.
    PublisherHandler(
        ros::NodeHandle&    nh,
        const std::string&  address,
        const unsigned int& buffer_size = 1,
        const bool&         latch = false,
        const double&       rate = 0
    )
    {
        if (rate != 0) {
            throw std::invalid_argument(
                "mrs_lib::PublisherHandler: a rate limit is not supported (" + address + ")"
            );
        }
        static_cast<void>(buffer_size);
        this->topic_ = nh.resolveName(address);
        this->output_ = &nh.host().template output<TopicType>(this->topic_, 16, latch);
        this->host_ = &nh.host();
    }

    void publish(const TopicType& msg)
    {
        if (this->output_ == nullptr) {
            throw std::logic_error(
                "mrs_lib::PublisherHandler: publish() before construction with a topic"
            );
        }
        if (!this->host_->inStep()) {
            throw std::logic_error(
                "mrs_lib::PublisherHandler: publish() outside a step (" + this->topic_ + ")"
            );
        }
        this->output_->send(msg);
    }

    void publish(const std::shared_ptr<TopicType>& msg)
    {
        this->publish(*msg);
    }

    void publish(const std::shared_ptr<TopicType const>& msg)
    {
        this->publish(*msg);
    }

    unsigned int getNumSubscribers()
    {
        return this->output_ != nullptr
                 ? static_cast<unsigned int>(this->output_->subscriberCount().value_or(1))
                 : 0;
    }

    std::string topicName() const
    {
        return this->topic_;
    }

  private:
    mrs::PluginHost*          host_{nullptr};
    hypha::Output<TopicType>* output_{nullptr};
    std::string               topic_;
};

}  // namespace mrs_lib
