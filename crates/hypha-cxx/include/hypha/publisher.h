#pragma once

#include "hypha/backend/session.h"
#include "hypha/backend/typed.h"
#include "hypha/hypha_ffi.h"
#include "hypha/options.h"
#include "hypha/status.h"
#include "hypha/topic.h"

#include <cstdint>
#include <expected>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>

namespace hypha {

class Node;

/// @brief Typed publisher. Node::advertise() creates one.
///
/// Threading: publish() runs on one thread at a time. Copies share one endpoint
/// and one `header.seq` counter.
template <typename M>
class Publisher
{
  public:
    /// @brief A disabled publisher: publish() sends nothing and returns false.
    Publisher() = default;

    /// @brief Publish a message. A `header.seq` field is sent as the next sequence number
    ///        of this publisher; the caller's message is not changed.
    /// @return True if it was sent; false if the publisher is disabled or the
    ///         transport refused the message.
    [[nodiscard]] bool publish(const M& message)
    {
        if (!this->handle) {
            this->lastStatus_ = HYPHA_STATUS_INVALID_STATE;
            return false;
        }
        this->lastStatus_ = this->handle->publish(message);
        return this->lastStatus_ == HYPHA_STATUS_OK;
    }

    /// @return The status of the most recent publish attempt.
    Status lastPublishStatus() const
    {
        return this->lastStatus_;
    }

    /// @brief False for an optionalTopic port whose table sets no topic.
    bool enabled() const
    {
        return static_cast<bool>(this->handle);
    }

    std::optional<size_t> subscriberCount() const
    {
        return this->handle ? this->handle->subscriberCount() : std::nullopt;
    }

    std::string topic() const
    {
        return this->handle ? this->handle->topic() : std::string();
    }

  private:
    friend class Node;

    /// @throws ConfigError when its port table is missing or refused, or the policy
    ///         mismatches (QosMismatch); std::runtime_error for any other refusal.
    Publisher(const detail::TopicSource& source, const detail::EndpointSettings& settings)
    {
        const std::string topic = source.resolveOrThrow();
        if (topic.empty()) {
            return;
        }
        require(validateTopicName(topic), source.describe());
        this->handle = std::make_shared<backend::PublisherHandle<M>>(
            topic, backend::typedPublisherOptions<M>(settings, 0)
        );
        require(Status(this->handle->creationStatus()), source.describe());
    }

    std::shared_ptr<backend::PublisherHandle<M>> handle;
    Status                                       lastStatus_{HYPHA_STATUS_OK};
};

}  // namespace hypha
