#pragma once

/// @file
/// Subscriber: the typed subscriber that the node's WaitSet drives.

#include "hypha/backend/config_readers.h"
#include "hypha/backend/endpoint_factory.h"
#include "hypha/backend/input_types.h"
#include "hypha/backend/introspection.h"
#include "hypha/backend/ros1_codec.h"
#include "hypha/backend/session.h"
#include "hypha/backend/subscriber_handle.h"
#include "hypha/backend/text_readers.h"
#include "hypha/backend/typed.h"
#include "hypha/hypha_ffi.h"
#include "hypha/options.h"
#include "hypha/raw.h"
#include "hypha/status.h"
#include "hypha/topic.h"
#include "hypha_base/clock.h"

#include <cstdint>
#include <expected>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace hypha {

/// @brief Owning pointer to a received message.
template <typename M>
using MessagePtr = typename backend::MessageTraits<M>::ConstPtr;

class Node;

/// @brief Typed subscriber that the node's WaitSet drives. Node::subscribe() creates one.
///
/// The run delivers every sample to the callback and keeps the newest one for
/// latestSample(). It delivers only frames whose schema hash is that of M.
///
/// A topic with `*` is a pattern: `*` is a whole segment (`/a/*`), `**` only the last
/// one (`/a/**`). It resolves once, at creation, to the topics that exist then.
template <typename M>
class Subscriber
{
  public:
    using ConstPtr = typename backend::MessageTraits<M>::ConstPtr;
    using Callback = std::function<void(const ConstPtr&)>;

  private:
    struct State;

    struct MessageForwarder
    {
        std::shared_ptr<State> state;

        void operator()(const ConstPtr& message, uint64_t transportSentNanos) const
        {
            if (this->state) {
                this->state->onMessage(message, transportSentNanos);
            }
        }
    };

  public:
    /// @brief A disabled subscriber: it never delivers.
    Subscriber() : shared(std::make_shared<State>())
    {
    }

    /// @brief False for an optionalTopic port whose table sets no topic.
    bool enabled() const
    {
        return this->shared->connected;
    }

    std::string topic() const
    {
        return this->shared->topicName;
    }

    /// The newest delivered message and its transport send time.
    struct LatestSample
    {
        ConstPtr message;
        Time     transportSent;
    };

    /// @brief The newest message and its transport send time, from one delivery.
    /// @return The sample, or NotAvailable before one arrives or when disabled.
    std::expected<LatestSample, Status> latestSample() const
    {
        const std::lock_guard<std::mutex> lock(this->shared->mutex);
        const bool                        nothingHeld = !this->handle || !this->shared->latest;
        if (nothingHeld) {
            return std::unexpected(Status::NotAvailable);
        }
        return LatestSample{this->shared->latest, this->shared->latestSent};
    }

    /// @brief Messages this subscription delivered. Unsigned, so it wraps.
    uint64_t messageCount() const
    {
        const std::lock_guard<std::mutex> lock(this->shared->mutex);
        return this->shared->count;
    }

    /// @brief Frames this subscriber lost before it read them, a queue overflow among them.
    ///        Any thread may read it while the node runs.
    /// @throws std::runtime_error when the library refuses the read.
    uint64_t missingSequences() const
    {
        return this->handle ? this->handle->missingSequences() : 0U;
    }

    /// @brief The counters of this subscriber. Any thread may read them while the node runs.
    /// @throws std::runtime_error when the library refuses the read.
    SubscriberMetrics metrics() const
    {
        return this->handle ? this->handle->counters() : SubscriberMetrics{};
    }

  private:
    friend class Node;

    /// @throws ConfigError when its port table is missing or refused, or the policy
    ///         mismatches (QosMismatch); std::runtime_error for any other refusal.
    Subscriber(
        const detail::TopicSource&      source,
        Callback                        callback,
        const detail::EndpointSettings& settings
    )
        : shared(std::make_shared<State>())
    {
        const std::string topic = source.resolveOrThrow();
        this->shared->userCallback = std::move(callback);
        if (topic.empty()) {
            return;
        }
        const bool isPattern = topic.find('*') != std::string::npos;
        if (!isPattern) {
            require(validateTopicName(topic), source.describe());
        }
        this->shared->topicName = topic;
        this->handle = std::make_shared<backend::SubscriberHandle>(require(
            backend::subscribeWithQos<M>(
                topic,
                backend::declaredSubscriberOptions(backend::policyOf<M>(settings.qos), 0),
                MessageForwarder{this->shared}
            ),
            source.describe()
        ));
        if (!isPattern) {
            const std::expected<void, std::string> claimed = backend::claimInputType(
                topic, codec::dataType<M>(), codec::md5Sum<M>(), source.describe(), this->handle
            );
            if (!claimed) {
                source.refuse("hypha: " + claimed.error());
            }
        }
        this->shared->connected = true;
        const std::expected<std::string, Status> resolved =
            backend::readSubscriberTopic(this->handle->rawSubscriber());
        if (resolved) {
            this->shared->topicName = *resolved;
        }
    }

    struct State
    {
        mutable std::mutex mutex;
        ConstPtr           latest;
        Time               latestSent;
        uint64_t           count{0};
        std::string        topicName;
        Callback           userCallback;
        bool               connected{false};

        void onMessage(const ConstPtr& message, uint64_t transportSentNanos)
        {
            {
                const std::lock_guard<std::mutex> lock(this->mutex);
                this->latest = message;
                this->latestSent = Time::fromNanos(static_cast<int64_t>(transportSentNanos));
                this->count += 1U;
            }
            // The callback never changes after attach, so it is called without the lock.
            if (this->userCallback) {
                this->userCallback(message);
            }
        }
    };

    std::shared_ptr<State>                     shared;
    std::shared_ptr<backend::SubscriberHandle> handle;
};

}  // namespace hypha
