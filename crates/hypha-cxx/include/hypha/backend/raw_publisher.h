#pragma once

#include "hypha/backend/handles.h"
#include "hypha/backend/session.h"
#include "hypha/hypha_ffi.h"
#include "hypha/raw.h"
#include "hypha_base/time.h"

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <expected>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <utility>
#include <vector>

namespace hypha::backend {

/// @return The number of subscribers of @p publisher, or no value when the transport
///         cannot count them.
inline std::optional<size_t> publisherSubscriberCount(const HyphaPublisher* publisher)
{
    size_t count = 0;
    if (hypha_publisher_subscriber_count(publisher, &count) == 0) {
        return std::nullopt;
    }
    return count;
}

class LoanedBuffer
{
  public:
    LoanedBuffer() = default;
    LoanedBuffer(const LoanedBuffer&) = delete;
    LoanedBuffer& operator=(const LoanedBuffer&) = delete;

    LoanedBuffer(LoanedBuffer&& other) noexcept
        : loan(std::move(other.loan))
        , writable(std::exchange(other.writable, nullptr))
        , length(std::exchange(other.length, 0))
        , frameworkSentNanos(std::exchange(other.frameworkSentNanos, -1))
    {
    }

    LoanedBuffer& operator=(LoanedBuffer&& other) noexcept
    {
        if (this != &other) {
            this->loan = std::move(other.loan);
            this->writable = std::exchange(other.writable, nullptr);
            this->length = std::exchange(other.length, 0);
            this->frameworkSentNanos = std::exchange(other.frameworkSentNanos, -1);
        }
        return *this;
    }

    uint8_t* data()
    {
        return this->writable;
    }

    size_t size() const
    {
        return this->length;
    }

    /// @brief Stamp the frame with framework time @p sent instead of the framework clock.
    /// @return HYPHA_STATUS_OK. HYPHA_STATUS_INVALID_STATE when the buffer holds no
    ///         loan, and HYPHA_STATUS_INVALID_ARGUMENT for a negative time.
    [[nodiscard]] Status setFrameworkSent(hypha::Time sent)
    {
        if (!this->loan) {
            return HYPHA_STATUS_INVALID_STATE;
        }
        if (sent.nanos() < 0) {
            return HYPHA_STATUS_INVALID_ARGUMENT;
        }
        this->frameworkSentNanos = sent.nanos();
        return HYPHA_STATUS_OK;
    }

    /// @brief Send only the first @p size bytes, for a message that came out shorter than
    ///        the buffer. The bytes stay where they are; data() stays valid.
    /// @return HYPHA_STATUS_OK. HYPHA_STATUS_INVALID_STATE when the buffer holds no loan,
    ///         and HYPHA_STATUS_INVALID_ARGUMENT for a size beyond size(); then nothing changes.
    [[nodiscard]] Status shrink(size_t size)
    {
        if (!this->loan) {
            return HYPHA_STATUS_INVALID_STATE;
        }
        const Status status = hypha_loan_shrink(this->loan.get(), size);
        if (status == HYPHA_STATUS_OK) {
            this->length = size;
        }
        return status;
    }

  private:
    friend class RawPublisherHandle;

    LoanedBuffer(HyphaLoan* handle, uint8_t* bytes, size_t size)
        : loan(handle), writable(bytes), length(size)
    {
    }

    HyphaLoan* release()
    {
        this->writable = nullptr;
        this->length = 0;
        return this->loan.release();
    }

    UniqueLoan loan;
    uint8_t*   writable{nullptr};
    size_t     length{0};
    /// The framework send stamp, or -1 for the framework clock at the send.
    int64_t frameworkSentNanos{-1};
};

/// Endpoint settings that declare no service policy. A zero @p maxPayloadBytes is growable.
inline HyphaPublisherOptions defaultPublisherQos(bool latch, uint64_t maxPayloadBytes = 0)
{
    HyphaPublisherOptions qos{};
    qos.latch = latch ? 1 : 0;
    qos.qos.max_payload_dynamic = maxPayloadBytes;
    return qos;
}

/// The delay before a subscriber with no publisher warns. Zero disables the warning.
inline constexpr int64_t DEFAULT_NO_PUBLISHER_WARNING_NANOS =
    static_cast<int64_t>(HYPHA_DEFAULT_NO_PUBLISHER_WARNING_NANOS);

/// Subscriber counterpart of defaultPublisherQos: NotAvailable until the topic exists.
inline HyphaSubscriberOptions defaultSubscriberQos(size_t queueSize)
{
    HyphaSubscriberOptions qos{};
    qos.depth = queueSize;
    qos.no_publisher_warning_after_nanos =
        static_cast<uint64_t>(DEFAULT_NO_PUBLISHER_WARNING_NANOS);
    return qos;
}

class RawPublisherHandle
{
  public:
    RawPublisherHandle() = default;

    RawPublisherHandle(
        std::string                  topic,
        std::string                  dataType,
        std::string                  md5,
        const std::string&           definition,
        const HyphaPublisherOptions& qos
    )
        : topicName(std::move(topic)), dataTypeName(std::move(dataType)), md5Sum(std::move(md5))
    {
        ffi::Session& state = ffi::session();
        HyphaNode*    node = nullptr;
        {
            const std::lock_guard<std::mutex> lock(state.mutex);
            node = state.node;
        }
        if (node == nullptr) {
            this->creation = HYPHA_STATUS_INVALID_STATE;
            return;
        }
        const Status status = hypha_publish_with_qos(
            node,
            this->topicName.data(),
            this->topicName.size(),
            this->dataTypeName.data(),
            this->dataTypeName.size(),
            this->md5Sum.data(),
            this->md5Sum.size(),
            definition.data(),
            definition.size(),
            &qos,
            this->publisher.put()
        );
        this->creation = status;
        this->valid = status == HYPHA_STATUS_OK;
    }

    RawPublisherHandle(const RawPublisherHandle&) = delete;
    RawPublisherHandle& operator=(const RawPublisherHandle&) = delete;

    RawPublisherHandle(RawPublisherHandle&& other) noexcept
    {
        this->take(std::move(other));
    }

    RawPublisherHandle& operator=(RawPublisherHandle&& other) noexcept
    {
        if (this != &other) {
            this->take(std::move(other));
        }
        return *this;
    }

    /// Send @p bytes as one frame.
    /// @return The status of the send.
    [[nodiscard]] Status publish(const std::vector<uint8_t>& bytes) const
    {
        if (!this->valid) {
            return HYPHA_STATUS_INVALID_STATE;
        }
        HyphaLoan*   loan = nullptr;
        uint8_t*     buffer = nullptr;
        const Status loanStatus =
            hypha_publisher_loan_frame(this->publisher.get(), bytes.size(), &loan, &buffer);
        if (loanStatus != HYPHA_STATUS_OK) {
            return loanStatus;
        }
        if (!bytes.empty()) {
            std::memcpy(buffer, bytes.data(), bytes.size());
        }
        // hypha_loan_send releases the loan whatever it returns.
        return hypha_loan_send(loan, -1);
    }

    /// @brief Borrow a buffer of @p size bytes to write one message into. Publish it
    ///        with publishLoan(); a buffer that goes out of scope is released unsent.
    [[nodiscard]] std::expected<LoanedBuffer, Status> loan(size_t size) const
    {
        if (!this->valid) {
            return std::unexpected(Status(HYPHA_STATUS_INVALID_STATE));
        }
        HyphaLoan*   loaned = nullptr;
        uint8_t*     bytes = nullptr;
        const Status loanStatus =
            hypha_publisher_loan_frame(this->publisher.get(), size, &loaned, &bytes);
        if (loanStatus != HYPHA_STATUS_OK) {
            return std::unexpected(Status(loanStatus));
        }
        return LoanedBuffer(loaned, bytes, size);
    }

    [[nodiscard]] Status publishLoan(LoanedBuffer&& loaned) const
    {
        if (!this->valid) {
            return HYPHA_STATUS_INVALID_STATE;
        }
        const int64_t stamp = loaned.frameworkSentNanos;
        HyphaLoan*    handle = loaned.release();
        if (handle == nullptr) {
            return HYPHA_STATUS_INVALID_ARGUMENT;
        }
        return hypha_loan_send(handle, stamp);
    }

    /// @return The number of subscribers on the topic, or no value when the publisher is
    ///         not valid.
    std::optional<size_t> subscriberCount() const
    {
        if (!this->valid) {
            return std::nullopt;
        }
        return publisherSubscriberCount(this->publisher.get());
    }

    bool isValid() const
    {
        return this->valid;
    }

    Status creationStatus() const
    {
        return this->creation;
    }

    /// Transfer endpoint ownership to an exclusively bound cycle output.
    HyphaPublisher* transfer()
    {
        this->valid = false;
        return this->publisher.release();
    }

    const std::string& topic() const
    {
        return this->topicName;
    }

  private:
    void take(RawPublisherHandle&& other)
    {
        this->topicName = std::move(other.topicName);
        this->dataTypeName = std::move(other.dataTypeName);
        this->md5Sum = std::move(other.md5Sum);
        this->publisher = std::move(other.publisher);
        this->valid = std::exchange(other.valid, false);
        this->creation = other.creation;
    }

    std::string     topicName;
    std::string     dataTypeName;
    std::string     md5Sum;
    UniquePublisher publisher;
    bool            valid{false};
    Status          creation{HYPHA_STATUS_INVALID_STATE};
};

}  // namespace hypha::backend
