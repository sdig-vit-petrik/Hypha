#pragma once

/// @file
/// The type-erased endpoints: RawPublisher, RawSubscriber, and the node and topic
/// introspection calls lookupDefinition and listNodes.

#include "hypha/backend/config_readers.h"
#include "hypha/backend/endpoint_factory.h"
#include "hypha/backend/introspection.h"
#include "hypha/backend/raw_publisher.h"
#include "hypha/backend/session.h"
#include "hypha/backend/subscriber_handle.h"
#include "hypha/backend/text_readers.h"
#include "hypha/introspection.h"
#include "hypha/options.h"
#include "hypha/qos.h"
#include "hypha/raw.h"
#include "hypha/status.h"
#include "hypha/topic.h"

#include <cstdint>
#include <expected>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

namespace hypha {

/// @brief Publishes raw bytes under an explicit type identity. A default-constructed one
///        reports InvalidState on each loan() and publish().
class RawPublisher
{
  public:
    RawPublisher() = default;

    explicit RawPublisher(backend::RawPublisherHandle handle)
        : held(std::make_shared<backend::RawPublisherHandle>(std::move(handle)))
    {
    }

    /// Send @p bytes as one frame.
    /// @return The status of the send; InvalidState for a publisher never created.
    [[nodiscard]] Status publish(const std::vector<uint8_t>& bytes) const
    {
        const bool usable = this->held && this->held->isValid();
        if (!usable) {
            return HYPHA_STATUS_INVALID_STATE;
        }
        return this->held->publish(bytes);
    }

    /// @return A writable shared-memory loan, or TooLarge above the payload limit, or the
    ///         refusing status when no buffer is free. Never blocks.
    [[nodiscard]] std::expected<backend::LoanedBuffer, Status> loan(size_t size) const
    {
        const bool usable = this->held && this->held->isValid();
        if (!usable) {
            return std::unexpected(Status::InvalidState);
        }
        return this->held->loan(size);
    }

    /// Send @p loaned, which it consumes: a second call with the same buffer returns
    /// InvalidArgument. A pointer copied out of data() is not valid after the send.
    [[nodiscard]] Status publishLoan(backend::LoanedBuffer&& loaned) const
    {
        const bool usable = this->held && this->held->isValid();
        if (!usable) {
            return HYPHA_STATUS_INVALID_STATE;
        }
        return this->held->publishLoan(std::move(loaned));
    }

    bool isValid() const
    {
        return this->held && this->held->isValid();
    }

    std::string topic() const
    {
        return this->held ? this->held->topic() : std::string();
    }

    /// @return The connected subscribers, or no value when the count is unavailable.
    std::optional<size_t> subscriberCount() const
    {
        return this->held ? this->held->subscriberCount() : std::nullopt;
    }

    /// @brief The status the library gave when it created the publisher.
    Status creationStatus() const
    {
        return this->held ? Status(this->held->creationStatus()) : Status(Status::InvalidState);
    }

  private:
    std::shared_ptr<backend::RawPublisherHandle> held;
};

/// @brief A writable buffer in shared transport memory.
using LoanedBuffer = backend::LoanedBuffer;

/// @brief The type a raw endpoint carries: its name, its MD5 and its definition text.
struct RawType
{
    std::string dataType;
    std::string md5;
    std::string definition;
};

/// @brief A publisher of raw bytes under the identity @p type. Options: latched,
///        maxPayloadDynamic(), a Qos.
///
/// Without a Qos it adopts the topic's service, or creates it with the `default` policy.
/// The maxPayloadDynamic of a Qos is not read here.
/// @return The publisher, or the status the library gave when it refused it.
template <PublisherOption... Options>
    requires(detail::FixedTopicOption<Options> && ...)
[[nodiscard]] std::expected<RawPublisher, Status>
rawPublisher(const std::string& topic, const RawType& type, Options... options)
{
    const detail::EndpointSettings settings =
        detail::settingsOf<detail::EndpointSettings>(options...);
    const uint64_t        payloadBound = settings.maxPayloadDynamic.value_or(0);
    HyphaPublisherOptions raw = backend::defaultPublisherQos(settings.latch, payloadBound);
    if (settings.qos.has_value()) {
        raw.qos = detail::declaredQos(*settings.qos, payloadBound);
    }
    RawPublisher created(
        backend::publishRawWithQos(topic, type.dataType, type.md5, type.definition, raw)
    );
    const Status creation = created.creationStatus();
    if (creation != Status::Ok) {
        return std::unexpected(creation);
    }
    return created;
}

/// @brief Receives every sample matching a topic pattern, as a `RawSample`.
class RawSubscriber
{
  public:
    RawSubscriber() = default;

    explicit RawSubscriber(backend::RawSubscriberHandle handle)
        : held(std::make_shared<backend::RawSubscriberHandle>(std::move(handle)))
    {
    }

    bool isValid() const
    {
        return this->held && this->held->isValid();
    }

    /// @brief Frames this subscriber lost before it read them, as
    ///        Subscriber::missingSequences(). Zero for a default-constructed subscriber.
    /// @throws std::runtime_error when the library refuses the read.
    uint64_t missingSequences() const
    {
        return this->held ? this->held->missingSequences() : 0U;
    }

    /// @brief The counters of this subscriber, as Subscriber::metrics(). All zero for a
    ///        default-constructed subscriber.
    /// @throws std::runtime_error when the library refuses the read.
    SubscriberMetrics metrics() const
    {
        return this->held ? this->held->counters() : SubscriberMetrics{};
    }

  private:
    std::shared_ptr<backend::RawSubscriberHandle> held;
};

/// @brief Subscribe to every topic matching @p pattern; it adopts the service of each topic.
/// @param depth Pending samples per topic before an older one is dropped. Zero is the
///        topic's maxDepth.
/// The pattern resolves once, at creation. A `*` must be a whole segment and `**` the
/// last one, else InvalidTopic.
/// @return The subscriber, or the refusal: InvalidState before a Node exists, NotAvailable
///         for a topic nobody has opened yet, ResourceLimit for a depth above maxDepth.
inline std::expected<RawSubscriber, Status> subscribeRaw(
    const std::string&                           pattern,
    std::function<void(const hypha::RawSample&)> callback,
    size_t                                       depth = 0
)
{
    std::expected<backend::SubscriberHandle, Status> attached =
        backend::subscribeRaw(pattern, std::move(callback), depth);
    if (!attached) {
        return std::unexpected(attached.error());
    }
    return RawSubscriber(std::move(*attached));
}

/// @brief Subscribe to the stream of @p type on @p topic, as raw bytes. A depth of zero is
///        the stream's maxDepth.
///
/// A topic carries one stream for each MD5, so the subscriber reads the messages of
/// `type.md5` alone; `type.definition` may be empty. Without @p qos it is NotAvailable
/// until the stream exists; with @p qos it can open it.
/// @return The subscriber, or the refusal status as for subscribeRaw(); QosMismatch
///         when the stream carries another policy than @p qos.
inline std::expected<RawSubscriber, Status> subscribeRawOfType(
    const std::string&                           topic,
    const RawType&                               type,
    std::function<void(const hypha::RawSample&)> callback,
    size_t                                       depth = 0,
    std::optional<Qos>                           qos = std::nullopt
)
{
    HyphaSubscriberOptions options = backend::defaultSubscriberQos(depth);
    if (qos.has_value()) {
        options.qos = detail::declaredQos(*qos, 0);
    }
    std::expected<backend::SubscriberHandle, Status> attached = backend::subscribeTypedWithQos(
        topic, type.dataType, std::move(callback), type.md5, type.definition, options
    );
    if (!attached) {
        return std::unexpected(attached.error());
    }
    return RawSubscriber(std::move(*attached));
}

/// @return The definition text for (topic, dataType, md5), or nothing while no publisher
///         of that identity has answered yet.
inline std::optional<std::string>
lookupDefinition(const std::string& topic, const std::string& dataType, const std::string& md5)
{
    return backend::lookupDefinition(topic, dataType, md5);
}

/// @return Every process the node registry knows about, alive or dead.
inline std::vector<NodeInfo> listNodes()
{
    return backend::listNodes();
}

}  // namespace hypha
