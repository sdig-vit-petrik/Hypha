#pragma once

#include "hypha/backend/raw_publisher.h"
#include "hypha/backend/session.h"
#include "hypha/backend/subscriber_handle.h"
#include "hypha/detail/report.h"
#include "hypha/hypha_ffi.h"
#include "hypha/raw.h"

#include <cstdio>
#include <expected>
#include <functional>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <utility>

namespace hypha::backend {

/// The one place a C ABI subscriber is opened.
inline ::HyphaStatus subscribeOnNode(
    HyphaNode*                        node,
    const std::string&                pattern,
    const std::optional<std::string>& dataType,
    const std::string&                md5,
    const std::string&                definition,
    const HyphaSubscriberOptions&     qos,
    HyphaSubscriber**                 out
)
{
    const char*  typeName = dataType.has_value() ? dataType->data() : nullptr;
    const size_t typeLength = dataType.has_value() ? dataType->size() : 0;
    return hypha_subscribe_typed(
        node,
        pattern.data(),
        pattern.size(),
        typeName,
        typeLength,
        md5.data(),
        md5.size(),
        definition.data(),
        definition.size(),
        &qos,
        out
    );
}

/// Open a subscriber that no WaitSet drives. An empty @p md5 expects no schema.
inline LoanedRawSubscriberHandle openLoanedSubscriber(
    const std::string&            pattern,
    const std::string&            dataType,
    const std::string&            md5,
    const std::string&            definition,
    const HyphaSubscriberOptions& qos
)
{
    ffi::Session& state = ffi::session();
    HyphaNode*    node = nullptr;
    {
        const std::lock_guard<std::mutex> lock(state.mutex);
        node = state.node;
    }
    if (node == nullptr) {
        return LoanedRawSubscriberHandle(HYPHA_STATUS_INVALID_ARGUMENT);
    }
    HyphaSubscriber* subscriber = nullptr;
    const Status     status = subscribeOnNode(
        node, pattern, std::optional<std::string>(dataType), md5, definition, qos, &subscriber
    );
    if (status != HYPHA_STATUS_OK) {
        return LoanedRawSubscriberHandle(status);
    }
    return LoanedRawSubscriberHandle(subscriber, status);
}

inline LoanedRawSubscriberHandle subscribeLoanedTypedWithQos(
    const std::string&            pattern,
    const std::string&            dataType,
    const std::string&            md5,
    const std::string&            definition,
    const HyphaSubscriberOptions& qos
)
{
    return openLoanedSubscriber(pattern, dataType, md5, definition, qos);
}

inline RawPublisherHandle publishRawWithQos(
    const std::string&           topic,
    const std::string&           dataType,
    const std::string&           md5,
    const std::string&           definition,
    const HyphaPublisherOptions& qos
)
{
    return RawPublisherHandle(topic, dataType, md5, definition, qos);
}

/// Attach a callback subscriber. An empty @p dataType means an untyped subscriber.
inline std::expected<SubscriberHandle, Status> attachSubscription(
    const std::string&                           pattern,
    std::function<void(const hypha::RawSample&)> callback,
    const std::optional<std::string>&            dataType,
    const std::string&                           md5,
    const std::string&                           definition,
    const HyphaSubscriberOptions&                qos
)
{
    ffi::Session& state = ffi::session();
    HyphaNode*    node = nullptr;
    {
        const std::lock_guard<std::mutex> lock(state.mutex);
        node = state.node;
    }
    if (node == nullptr) {
        return std::unexpected(Status::InvalidState);
    }

    std::shared_ptr<ffi::Subscription> held = std::make_shared<ffi::Subscription>();
    held->pattern = pattern;
    held->callback = std::move(callback);

    const Status status =
        subscribeOnNode(node, pattern, dataType, md5, definition, qos, held->subscriber.put());
    if (status != HYPHA_STATUS_OK) {
        return std::unexpected(status);
    }

    HyphaWaitSet* waitset = defaultWaitSet();
    if (waitset == nullptr) {
        held->subscriber.reset();
        return std::unexpected(Status::InvalidState);
    }
    // The registration retains the subscription, so a callback may drop its own handle.
    ffi::SubscriptionPtr* retained = new ffi::SubscriptionPtr(held);
    HyphaAttachment       record{};
    record.kind = HYPHA_ATTACH_SUBSCRIPTION;
    record.subscriber = held->subscriber.get();
    record.subscription_callback = &ffi::deliver;
    record.context = retained;
    record.release = &ffi::releaseSubscription;
    const Status registerStatus = hypha_waitset_attach(waitset, &record, &held->executorId);
    if (registerStatus != HYPHA_STATUS_OK) {
        // A failed attach transfers no context, so this side releases it.
        delete retained;
        held->subscriber.reset();
        hypha::detail::error(
            "subscription '" + pattern + "' was refused by the WaitSet: status "
            + std::to_string(registerStatus.asInt())
        );
        return std::unexpected(registerStatus);
    }
    return SubscriberHandle(held);
}

/// Untyped callback subscriber with @p queueSize pending samples.
inline std::expected<SubscriberHandle, Status> subscribeRaw(
    const std::string&                           pattern,
    std::function<void(const hypha::RawSample&)> callback,
    size_t                                       queueSize
)
{
    return attachSubscription(
        pattern, std::move(callback), std::nullopt, "", "", defaultSubscriberQos(queueSize)
    );
}

inline std::expected<SubscriberHandle, Status> subscribeTypedWithQos(
    const std::string&                           pattern,
    const std::string&                           dataType,
    std::function<void(const hypha::RawSample&)> callback,
    const std::string&                           md5,
    const std::string&                           definition,
    const HyphaSubscriberOptions&                qos
)
{
    return attachSubscription(pattern, std::move(callback), dataType, md5, definition, qos);
}

}  // namespace hypha::backend
