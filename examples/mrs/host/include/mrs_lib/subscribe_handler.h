#pragma once

/// @file
/// mrs_lib::SubscribeHandler over a port of the host's cycle (hypha port of the
/// upstream header). Without a callback it reads a Latest port; with one, an Arrivals
/// port whose callback runs in the host's step once per sample, oldest first.
/// No-message timeouts are not watched: the port's verdict and hasMsg() tell the code.

#include <ros/node_handle.h>
#include <ros/ros.h>

#include <functional>
#include <memory>
#include <optional>
#include <string>

namespace mrs_lib {

static const ros::Duration no_timeout = ros::Duration(0, 0);

struct SubscribeHandlerOptions
{
    SubscribeHandlerOptions(const ros::NodeHandle& nh) : nh(nh)  // NOLINT: implicit, as upstream
    {
    }

    SubscribeHandlerOptions() = default;

    ros::NodeHandle nh;
    std::string     node_name = {};
    std::string     topic_name = {};
    ros::Duration   no_message_timeout = mrs_lib::no_timeout;
    std::function<void(const std::string&, const ros::Time&)> timeout_callback = {};
    bool                                                      threadsafe = true;
    bool                                                      autostart = true;
    uint32_t                                                  queue_size = 3;
    ros::TransportHints transport_hints = ros::TransportHints();
};

template <typename MessageType>
class SubscribeHandler
{
  public:
    using ConstPtr = typename MessageType::ConstPtr;
    using Callback = std::function<void(const ConstPtr&)>;

    SubscribeHandler() = default;

    SubscribeHandler(const SubscribeHandlerOptions& options, const std::string& topic_name)
        : SubscribeHandler(options, topic_name, Callback{})
    {
    }

    SubscribeHandler(const SubscribeHandlerOptions& options)
        : SubscribeHandler(options, options.topic_name, Callback{})
    {
    }

    SubscribeHandler(
        const SubscribeHandlerOptions& options, const std::string& topic_name, Callback callback
    )
        : state_(std::make_shared<State>())
    {
        mrs::PluginHost& host = options.nh.host();
        this->state_->host = &host;
        this->state_->topic = options.nh.resolveName(topic_name);
        this->state_->running = options.autostart;
        // Without a callback the newest sample is read from a Latest port, unless the host
        // has an Arrivals port for the topic (the manager's trigger): then from that.
        if (!callback && !host.hasArrivals(this->state_->topic)) {
            this->state_->latest = &host.latest<MessageType>(this->state_->topic);
            return;
        }
        std::weak_ptr<State> weak = this->state_;
        host.onArrival<MessageType>(
            this->state_->topic, [weak, callback](const MessageType& message) {
                const std::shared_ptr<State> state = weak.lock();
                if (!state || !state->running) {
                    return;
                }
                state->last = std::make_shared<const MessageType>(message);
                state->lastTime = ros::Time::now();
                state->fresh = true;
                if (callback) {
                    callback(state->last);
                }
            }
        );
    }

    template <class ObjectType>
    SubscribeHandler(
        const SubscribeHandlerOptions& options,
        const std::string&             topic_name,
        void (ObjectType::*callback)(const ConstPtr&),
        ObjectType* object
    )
        : SubscribeHandler(options, topic_name, Callback([object, callback](const ConstPtr& msg) {
                               (object->*callback)(msg);
                           }))
    {
    }

    /// The newest message, and it is no longer new.
    ConstPtr getMsg()
    {
        this->refresh();
        this->state().fresh = false;
        return this->state().last;
    }

    ConstPtr peekMsg() const
    {
        const_cast<SubscribeHandler*>(this)->refresh();
        return this->state().last;
    }

    bool hasMsg() const
    {
        const_cast<SubscribeHandler*>(this)->refresh();
        return this->state().last != nullptr;
    }

    bool newMsg() const
    {
        const_cast<SubscribeHandler*>(this)->refresh();
        return this->state().fresh;
    }

    bool usedMsg() const
    {
        return this->hasMsg() && !this->newMsg();
    }

    ros::Time lastMsgTime() const
    {
        const_cast<SubscribeHandler*>(this)->refresh();
        return this->state().lastTime;
    }

    std::string topicName() const
    {
        return this->state().topic;
    }

    std::string subscribedTopicName() const
    {
        return this->state().topic;
    }

    void start()
    {
        this->state().running = true;
    }

    void stop()
    {
        this->state().running = false;
    }

  private:
    struct State
    {
        mrs::PluginHost*            host{nullptr};
        std::string                 topic;
        hypha::Latest<MessageType>* latest{nullptr};
        bool                        running{true};
        ConstPtr                    last;
        ros::Time                   lastTime;
        bool                        fresh{false};
        uint64_t                    seen{0};
    };

    State& state() const
    {
        if (!this->state_) {
            throw std::logic_error(
                "mrs_lib::SubscribeHandler: used before construction with a topic"
            );
        }
        return *this->state_;
    }

    /// A Latest port: take its held value when a new one arrived since the last look.
    void refresh()
    {
        State& state = this->state();
        if (state.latest == nullptr || !state.running || !state.latest->has()) {
            return;
        }
        const uint64_t received = state.latest->received();
        if (received == state.seen && state.last) {
            return;
        }
        state.seen = received;
        // The newest value on purpose: the handler hands out its last message.
        state.last = std::make_shared<const MessageType>(state.latest->lastKnown());
        state.lastTime = ros::Time::now();
        state.fresh = true;
    }

    std::shared_ptr<State> state_;
};

}  // namespace mrs_lib
