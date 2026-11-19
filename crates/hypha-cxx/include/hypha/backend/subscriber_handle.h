#pragma once

#include "hypha/backend/handles.h"
#include "hypha/backend/session.h"
#include "hypha/detail/report.h"
#include "hypha/hypha_ffi.h"

#include <atomic>
#include <memory>
#include <mutex>
#include <string>
#include <utility>

namespace hypha::backend {

class SubscriberHandle
{
  public:
    SubscriberHandle() = default;

    explicit SubscriberHandle(ffi::SubscriptionPtr held) : subscription(std::move(held))
    {
    }

    SubscriberHandle(const SubscriberHandle&) = delete;
    SubscriberHandle& operator=(const SubscriberHandle&) = delete;

    SubscriberHandle(SubscriberHandle&& other) noexcept
        : subscription(std::move(other.subscription))
    {
        other.subscription.reset();
    }

    // Release first: the executor would otherwise keep a pointer into freed memory.
    SubscriberHandle& operator=(SubscriberHandle&& other) noexcept
    {
        if (this != &other) {
            this->release();
            this->subscription = std::move(other.subscription);
            other.subscription.reset();
        }
        return *this;
    }

    ~SubscriberHandle()
    {
        this->release();
    }

    bool isValid() const
    {
        return this->subscription && this->subscription->active.load(std::memory_order_acquire);
    }

    HyphaSubscriber* rawSubscriber() const
    {
        return this->subscription ? this->subscription->subscriber.get() : nullptr;
    }

    /// @return The frames this subscriber lost before it read them, or zero for an empty
    ///         handle.
    /// @throws std::runtime_error when the library refuses the read of a live handle.
    uint64_t missingSequences() const
    {
        const HyphaSubscriber* subscriber = this->rawSubscriber();
        if (subscriber == nullptr) {
            return 0;
        }
        uint64_t count = 0;
        require(
            Status(hypha_subscriber_missing_sequences(subscriber, &count)),
            "subscriber missing sequences"
        );
        return count;
    }

    /// @return The counters of this subscriber, or all zero for an empty handle.
    /// @throws std::runtime_error when the library refuses the read of a live handle.
    hypha::SubscriberMetrics counters() const
    {
        hypha::SubscriberMetrics metrics{};
        const HyphaSubscriber*   subscriber = this->rawSubscriber();
        if (subscriber == nullptr) {
            return metrics;
        }
        HyphaSubscriberCounters raw{};
        require(Status(hypha_subscriber_counters(subscriber, &raw)), "subscriber counters");
        metrics.sequenceGap = raw.sequence_gap;
        metrics.invalidFrame = raw.invalid_frame;
        metrics.corruptPayload = raw.corrupt_payload;
        metrics.schemaMismatch = raw.schema_mismatch;
        metrics.invalidTimestamp = raw.invalid_timestamp;
        metrics.loanBudget = raw.loan_budget;
        metrics.loanHeld = raw.loan_held;
        metrics.loanRefused = raw.loan_refused;
        return metrics;
    }

  private:
    /// Remove the executor registration, then give up the held state: the executor
    /// borrows the subscription by raw pointer.
    void release()
    {
        if (!this->subscription) {
            return;
        }
        this->subscription->active.store(false, std::memory_order_release);
        ffi::Session& state = ffi::session();
        HyphaNode*    node = nullptr;
        {
            const std::lock_guard<std::mutex> lock(state.mutex);
            node = state.node;
        }
        const bool isRegistered = this->subscription->executorId != 0;
        if (node != nullptr) {
            if (isRegistered) {
                HyphaWaitSet* waitset = defaultWaitSet();
                if (waitset != nullptr) {
                    const ::HyphaStatus status =
                        hypha_waitset_detach(waitset, this->subscription->executorId);
                    if (status != HYPHA_STATUS_OK) {
                        hypha::detail::error(
                            "a subscription could not detach its registration: status "
                            + std::to_string(static_cast<int>(status))
                            + "; the registration stays until the WaitSet is destroyed"
                        );
                    }
                }
                this->subscription->executorId = 0;
            }
        }
        this->subscription.reset();
    }

    ffi::SubscriptionPtr subscription;
};

class LoanedRawSubscriberHandle
{
  public:
    LoanedRawSubscriberHandle() = default;

    explicit LoanedRawSubscriberHandle(Status initialStatus)
        : subscriber(nullptr), lastStatus(initialStatus)
    {
    }

    explicit LoanedRawSubscriberHandle(
        HyphaSubscriber* value, Status initialStatus = HYPHA_STATUS_OK
    )
        : subscriber(value), lastStatus(initialStatus)
    {
    }

    bool isValid() const
    {
        return static_cast<bool>(this->subscriber);
    }

    /// Transfer endpoint ownership to a managed input source.
    HyphaSubscriber* release()
    {
        return this->subscriber.release();
    }

    Status status() const
    {
        return this->lastStatus;
    }

  private:
    UniqueSubscriber subscriber;
    Status           lastStatus{HYPHA_STATUS_OK};
};

using RawSubscriberHandle = SubscriberHandle;

}  // namespace hypha::backend
