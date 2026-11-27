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

    /// @brief Reserve the slot of one message whose large field is written in place: its
    ///        single top-level array of numbers (`Image::data`, `PointCloud2::data`).
    ///
    /// @p head holds the small fields, its loaned field empty; the lengths of its strings
    /// and arrays are final. The Loan's data() is @p capacityBytes of the slot: fill it with
    /// std::memcpy or Loan::put(), then publish(). Nothing waits, and a refusal is no lost
    /// frame, so the caller may ask again: PoolExhausted for a full pool, LoanLimit while
    /// the caller holds all of this publisher's loans (two), TooLarge past the payload
    /// limit. A Loan dropped unsent returns its slot and takes no `header.seq`.
    /// @return The loan, or the refusal; lastPublishStatus() says the same.
    [[nodiscard]] std::expected<Loan<M>, Status> loan(M head, size_t capacityBytes)
    {
        if (!this->handle) {
            this->lastStatus_ = HYPHA_STATUS_INVALID_STATE;
            return std::unexpected(this->lastStatus_);
        }
        std::expected<Loan<M>, Status> loaned = this->handle->loan(std::move(head), capacityBytes);
        this->lastStatus_ = loaned.has_value() ? Status(HYPHA_STATUS_OK) : loaned.error();
        return loaned;
    }

    /// @brief Send @p loan with all capacity() bytes of its loaned field. The constraint
    ///        keeps this overload out of publish(const M&) for every other type.
    [[nodiscard]] bool publish(Loan<M>&& loan)
        requires LoanableMessage<M>
    {
        const size_t capacity = loan.capacity();
        return this->publish(std::move(loan), capacity);
    }

    /// @brief Send @p loan with the first @p usedBytes of its loaned field, at most
    ///        capacity(): a cloud whose point count is known only at the end. The bytes on
    ///        the wire are those publish(const M&) sends for the same message, with the next
    ///        `header.seq`. InvalidArgument, nothing sent and no `header.seq` taken, for a
    ///        loan of another publisher, a moved-from loan, a size that is not whole elements
    ///        or is past capacity(), a filled loaned field, or a head whose strings or
    ///        arrays changed length.
    /// @return True if it was sent. The loan is consumed either way.
    [[nodiscard]] bool publish(Loan<M>&& loan, size_t usedBytes)
    {
        if (!this->handle) {
            this->lastStatus_ = HYPHA_STATUS_INVALID_STATE;
            return false;
        }
        Loan<M> consumed = std::move(loan);
        this->lastStatus_ = this->handle->publish(std::move(consumed), usedBytes);
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
