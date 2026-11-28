#pragma once

/// @file
/// The typed publish/subscribe surface built over the C ABI raw endpoints.

#include "hypha/backend/endpoint_factory.h"
#include "hypha/backend/ros1_codec.h"
#include "hypha/detail/report.h"
#include "hypha/loan.h"
#include "hypha/options.h"
#include "hypha/qos.h"
#include "hypha/raw.h"
#include "hypha/status.h"
#include "hypha/view.h"

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <expected>
#include <functional>
#include <memory>
#include <optional>
#include <string>

namespace hypha::backend {

template <typename M>
struct MessageTraits
{
    using ConstPtr = std::shared_ptr<const M>;
};

/// A subscriber of views receives ViewPtr<M>, which reads the sample in place.
template <typename M>
struct MessageTraits<View<M>>
{
    using ConstPtr = ViewPtr<M>;
};

/// @brief The policy a typed endpoint of messages @p M declares: its Qos option, or
///        QosOf<M>::value.
template <typename M>
Qos policyOf(const std::optional<Qos>& option)
{
    if (option.has_value()) {
        return *option;
    }
    return QosOf<M>::value;
}

/// @brief The payload bound a publisher passes: the maxPayloadDynamic() option, or that of
///        @p policy. Zero is growable.
inline uint64_t payloadBoundOf(const hypha::detail::EndpointSettings& settings, const Qos& policy)
{
    if (settings.maxPayloadDynamic.has_value()) {
        return *settings.maxPayloadDynamic;
    }
    return policy.maxPayloadDynamic;
}

/// @brief The options of a typed publisher of @p M that holds @p loanedSamples loans at
///        once (zero: the library's default).
template <typename M>
HyphaPublisherOptions
typedPublisherOptions(const hypha::detail::EndpointSettings& settings, uint32_t loanedSamples)
{
    const Qos             policy = policyOf<M>(settings.qos);
    HyphaPublisherOptions options{};
    options.latch = settings.latch ? 1 : 0;
    options.loaned_samples = loanedSamples;
    options.qos = hypha::detail::declaredQos(policy, payloadBoundOf(settings, policy));
    return options;
}

/// @brief The options of a subscriber that declares @p policy and asks for a queue of
///        @p depth samples. Zero is the deepest queue the policy allows.
inline HyphaSubscriberOptions declaredSubscriberOptions(const Qos& policy, size_t depth)
{
    HyphaSubscriberOptions options = defaultSubscriberQos(depth);
    options.qos = hypha::detail::declaredQos(policy, 0);
    return options;
}

/// @brief Typed publisher that encodes into shared transport memory.
template <typename M>
class PublisherHandle
{
  public:
    PublisherHandle() = default;

    PublisherHandle(std::string topic, const HyphaPublisherOptions& qos)
        : raw(hypha::backend::publishRawWithQos(
              topic, codec::dataType<M>(), codec::md5Sum<M>(), codec::definition<M>(), qos
          ))
    {
    }

    /// @brief Encode and send. A message with `header.seq` gets the next sequence number
    ///        of this endpoint, shared by every Publisher copy that holds it.
    [[nodiscard]] Status publish(const M& message)
    {
        const size_t                        length = codec::encodedLength(message);
        std::expected<LoanedBuffer, Status> loaned = this->raw.loan(length);
        if (!loaned.has_value()) {
            return loaned.error();
        }
        // The unchecked encode needs a loan of exactly the length just taken.
        if (loaned->size() != length
            || !codec::detail::encodeWithSeq(message, loaned->data(), length, this->seq_++)) {
            return HYPHA_STATUS_INVALID_SCHEMA;
        }
        return this->raw.publishLoan(std::move(*loaned));
    }

    /// @brief Reserve a slot for @p head with @p capacity bytes in its loaned field, which
    ///        must be empty. A refusal takes no sequence. See Publisher<M>::loan().
    [[nodiscard]] std::expected<Loan<M>, Status> loan(M head, size_t capacity)
    {
        using Element = typename Loan<M>::Element;
        const auto& field = head.*message_traits::LoanedField<M>::member;
        const bool  refused = !field.empty() || capacity % sizeof(Element) != 0
                           || capacity / sizeof(Element) > UINT32_MAX;
        if (refused) {
            return std::unexpected(Status(HYPHA_STATUS_INVALID_ARGUMENT));
        }
        detail::GapStream<detail::LoanedFieldType<M>> measured(
            nullptr, static_cast<size_t>(-1), &field, 0, capacity
        );
        ros::serialization::serialize(measured, head);
        if (!measured.complete()) {
            return std::unexpected(Status(HYPHA_STATUS_INVALID_SCHEMA));
        }
        std::expected<LoanedBuffer, Status> reserved = this->raw.reserve(measured.length());
        if (!reserved.has_value()) {
            return std::unexpected(reserved.error());
        }
        return Loan<M>(
            std::move(head),
            std::move(*reserved),
            this,
            measured.gapOffset(),
            capacity,
            measured.length() - capacity
        );
    }

    /// @brief Encode the small fields around the first @p used bytes of the loaned field and
    ///        send them, in one pass over the head. See Publisher<M>::publish(Loan<M>&&, size_t).
    [[nodiscard]] Status publish(Loan<M>&& loan, size_t used)
    {
        if (loan.owner_ != this) {
            return HYPHA_STATUS_INVALID_ARGUMENT;
        }
        // The sequence is taken only once nothing can refuse the frame but the send.
        const std::expected<size_t, Status> total = loan.seal(used, [this] {
            return this->seq_++;
        });
        if (!total.has_value()) {
            return total.error();
        }
        if (*total < loan.buffer_.size()) {
            const Status shrunk = loan.buffer_.shrink(*total);
            if (shrunk != HYPHA_STATUS_OK) {
                return shrunk;
            }
        }
        return this->raw.publishLoan(std::move(loan.buffer_));
    }

    std::optional<size_t> subscriberCount() const
    {
        return this->raw.subscriberCount();
    }

    bool isValid() const
    {
        return this->raw.isValid();
    }

    Status creationStatus() const
    {
        return this->raw.creationStatus();
    }

    const std::string& topic() const
    {
        return this->raw.topic();
    }

    HyphaPublisher* transfer()
    {
        return this->raw.transfer();
    }

  private:
    RawPublisherHandle    raw;
    std::atomic<uint32_t> seq_{0};
};

/// @brief Typed subscribe: decodes a RawSample, then calls @p callback. A payload that
///        does not deserialize is dropped and logged at a bounded rate.
template <typename M>
struct TypedSampleHandler
{
    /// Receives the message and the transport send time of its frame.
    std::function<void(const typename MessageTraits<M>::ConstPtr&, uint64_t)> callback;
    std::shared_ptr<hypha::detail::RecurringFault>                            undecodable;

    void operator()(const hypha::RawSample& sample) const
    {
        std::shared_ptr<M> decoded = std::make_shared<M>();
        if (!codec::decode(sample.bytes.data(), sample.bytes.size(), *decoded)) {
            this->undecodable->note(
                "dropping a sample on '" + std::string(sample.topic)
                + "': payload failed to deserialize as " + codec::dataType<M>()
            );
            return;
        }
        this->callback(std::move(decoded), static_cast<uint64_t>(sample.transportSent.nanos()));
    }
};

/// Views of the sample in place: the callback reads the slot, and copies it still holds at
/// the return keep it within the subscriber's loan budget, or own a copy of its bytes.
template <typename M>
struct TypedSampleHandler<View<M>>
{
    std::function<void(const ViewPtr<M>&, uint64_t)> callback;
    std::shared_ptr<hypha::detail::RecurringFault>   undecodable;
    std::shared_ptr<hypha::detail::RecurringFault>   copied =
        std::make_shared<hypha::detail::RecurringFault>();

    void operator()(const hypha::RawSample& sample) const
    {
        ViewPtr<M> view =
            hypha::detail::ViewAccess<M>::lend(sample.bytes.data(), sample.bytes.size());
        if (!view) {
            this->undecodable->note(
                "dropping a sample on '" + std::string(sample.topic)
                + "': payload failed to deserialize as " + codec::dataType<M>()
            );
            return;
        }
        try {
            this->callback(view, static_cast<uint64_t>(sample.transportSent.nanos()));
        } catch (...) {
            this->settle(view, sample);
            throw;
        }
        this->settle(view, sample);
    }

    void settle(ViewPtr<M>& view, const hypha::RawSample& sample) const
    {
        const bool copiedBytes =
            hypha::detail::ViewAccess<M>::settle(view, [&sample](HyphaReceivedSample*& kept) {
                return sample.delivery != nullptr
                    && hypha_delivery_retain(sample.delivery, &kept) == HYPHA_STATUS_OK;
            });
        if (copiedBytes) {
            this->copied->note(
                "a view of '" + std::string(sample.topic)
                + "' outlived its callback past the subscriber's loan budget; its bytes were "
                  "copied"
            );
        }
    }
};

template <typename M>
std::expected<SubscriberHandle, Status> subscribeWithQos(
    const std::string&                                                        topic,
    const HyphaSubscriberOptions&                                             qos,
    std::function<void(const typename MessageTraits<M>::ConstPtr&, uint64_t)> callback
)
{
    TypedSampleHandler<M> handler{
        std::move(callback), std::make_shared<hypha::detail::RecurringFault>()
    };
    return hypha::backend::subscribeTypedWithQos(
        topic,
        codec::dataType<M>(),
        std::move(handler),
        codec::md5Sum<M>(),
        codec::definition<M>(),
        qos
    );
}

}  // namespace hypha::backend
