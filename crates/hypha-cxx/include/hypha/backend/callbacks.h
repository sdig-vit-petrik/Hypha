#pragma once

/// @file
/// The named trampolines that the executor calls: delivery and timers.

#include "hypha/backend/session_state.h"
#include "hypha/hypha_ffi.h"
#include "hypha/raw.h"
#include "hypha/status.h"
#include "hypha_base/clock.h"

#include <atomic>
#include <cstdint>
#include <memory>
#include <span>
#include <string>
#include <string_view>

namespace hypha::backend {

namespace ffi {

/// What a callback returns when it wants the run to continue.
inline HyphaCallbackResult continueRun() noexcept
{
    HyphaCallbackResult result{};
    result.action = HYPHA_ACTION_CONTINUE;
    result.status = 0;
    return result;
}

/// What a callback returns when the run must fail with @p status.
inline HyphaCallbackResult failRun(::HyphaStatus status) noexcept
{
    HyphaCallbackResult result{};
    result.action = HYPHA_ACTION_FAIL;
    result.status = static_cast<int32_t>(status);
    return result;
}

/// Releases the strong reference a registration holds on a subscription, once no callback
/// can run again.
inline void releaseSubscription(void* context) noexcept
{
    delete static_cast<ffi::SubscriptionPtr*>(context);
}

/// Crosses the C boundary, so it never throws. `delivery->payload` is null when the
/// sample did not parse as an hypha frame.
inline HyphaCallbackResult deliver(const HyphaDelivery* delivery, void* userData) noexcept
{
    ffi::SubscriptionPtr* retained = static_cast<ffi::SubscriptionPtr*>(userData);
    const bool            released = retained == nullptr || !*retained;
    if (released) {
        return continueRun();
    }
    ffi::Subscription* subscription = retained->get();
    // A caller that released its handle set this false.
    const bool isActive = subscription->active.load(std::memory_order_acquire);
    const bool cannotDeliver = !isActive || !subscription->callback;
    if (cannotDeliver) {
        return continueRun();
    }
    if (delivery == nullptr) {
        return continueRun();
    }
    if (delivery->payload == nullptr) {
        subscription->notAFrame.note(
            "dropping a sample on '" + subscription->pattern + "': payload is not an hypha frame"
        );
        return continueRun();
    }
    try {
        hypha::RawSample sample;
        sample.topic = std::string_view(delivery->identity.topic, delivery->identity.topic_len);
        sample.schemaHash = delivery->identity.schema_hash;
        sample.bytes = std::span<const uint8_t>(delivery->payload, delivery->payload_len);
        sample.transportSent =
            hypha::Time::fromNanos(static_cast<int64_t>(delivery->identity.transport_sent_nanos));
        sample.frameworkSent =
            hypha::Time::fromNanos(static_cast<int64_t>(delivery->identity.framework_sent_nanos));
        sample.transportAge =
            hypha::Duration::fromNanos(static_cast<int64_t>(delivery->transport_age_nanos));
        subscription->callback(sample);
    } catch (...) {
        subscription->callbackFailures.note(
            "a subscription callback on '" + subscription->pattern + "' threw; the run failed"
        );
        return failRun(HYPHA_STATUS_PANIC);
    }
    return continueRun();
}

/// Releases the registration's reference to a timer state, once no callback can run again.
inline void releaseTimerAttachment(void* userData) noexcept
{
    ffi::TimerState* timer = static_cast<ffi::TimerState*>(userData);
    if (timer == nullptr) {
        return;
    }
    timer->attachmentRef.reset();
}

inline HyphaCallbackResult fireTimer(uint64_t missedPeriods, void* userData) noexcept
{
    ffi::TimerState* timer = static_cast<ffi::TimerState*>(userData);
    if (timer == nullptr) {
        return continueRun();
    }
    // A handle whose detach was refused leaves the registration in place; stay silent.
    if (!timer->active.load(std::memory_order_acquire)) {
        return continueRun();
    }
    timer->missed.fetch_add(missedPeriods, std::memory_order_relaxed);
    if (!timer->callback) {
        return continueRun();
    }
    try {
        timer->callback();
    } catch (...) {
        timer->callbackFailures.note("a timer callback threw; the run failed");
        return failRun(HYPHA_STATUS_PANIC);
    }
    return continueRun();
}

}  // namespace ffi

}  // namespace hypha::backend
