#pragma once

#include "hypha/backend/session.h"
#include "hypha/detail/report.h"
#include "hypha/hypha_ffi.h"
#include "hypha_base/clock.h"

#include <atomic>
#include <cstdint>
#include <expected>
#include <functional>
#include <memory>
#include <mutex>
#include <string>
#include <utility>

namespace hypha::backend {

class TimerHandle
{
  public:
    TimerHandle() = default;

    explicit TimerHandle(ffi::TimerPtr held) : timer(std::move(held))
    {
    }

    TimerHandle(const TimerHandle&) = delete;
    TimerHandle& operator=(const TimerHandle&) = delete;

    TimerHandle(TimerHandle&& other) noexcept : timer(std::move(other.timer))
    {
        other.timer.reset();
    }

    // Release first: the executor would otherwise keep a pointer into freed memory.
    TimerHandle& operator=(TimerHandle&& other) noexcept
    {
        if (this != &other) {
            this->release();
            this->timer = std::move(other.timer);
            other.timer.reset();
        }
        return *this;
    }

    ~TimerHandle()
    {
        this->release();
    }

    bool isValid() const
    {
        return static_cast<bool>(this->timer)
            && this->timer->active.load(std::memory_order_acquire);
    }

    uint64_t missedCount() const
    {
        return this->timer ? this->timer->missed.load(std::memory_order_relaxed) : 0U;
    }

    /// `hypha_waitset_timer_cancel` on this registration.
    std::expected<void, Status> cancel() const
    {
        return this->control(&hypha_waitset_timer_cancel);
    }

    /// `hypha_waitset_timer_reset` on this registration.
    std::expected<void, Status> reset() const
    {
        return this->control(&hypha_waitset_timer_reset);
    }

    /// `hypha_waitset_timer_state` of this registration.
    std::expected<HyphaTimerState, Status> state() const
    {
        HyphaWaitSet* waitset = this->registeredWaitSet();
        if (waitset == nullptr) {
            return std::unexpected(Status::InvalidState);
        }
        HyphaTimerState out{};
        const Status    status = hypha_waitset_timer_state(waitset, this->timer->executorId, &out);
        if (status != HYPHA_STATUS_OK) {
            return std::unexpected(status);
        }
        return out;
    }

  private:
    /// The WaitSet of a live registration, or null: no timer, a released one, or no node.
    HyphaWaitSet* registeredWaitSet() const
    {
        if (!this->isValid() || this->timer->executorId == 0) {
            return nullptr;
        }
        return defaultWaitSet();
    }

    std::expected<void, Status> control(HyphaStatus (*call)(HyphaWaitSet*, HyphaAttachmentId)) const
    {
        HyphaWaitSet* waitset = this->registeredWaitSet();
        if (waitset == nullptr) {
            return std::unexpected(Status::InvalidState);
        }
        const Status status = call(waitset, this->timer->executorId);
        if (status != HYPHA_STATUS_OK) {
            return std::unexpected(status);
        }
        return {};
    }

    /// Remove the executor registration, then give up the held state: the executor
    /// borrows the state by raw pointer.
    void release()
    {
        if (!this->timer) {
            return;
        }
        this->timer->active.store(false, std::memory_order_release);
        ffi::Session& state = ffi::session();
        HyphaNode*    node = nullptr;
        {
            const std::lock_guard<std::mutex> lock(state.mutex);
            node = state.node;
        }
        const bool isRegistered = this->timer->executorId != 0;
        if (node != nullptr) {
            if (isRegistered) {
                HyphaWaitSet* waitset = defaultWaitSet();
                if (waitset != nullptr) {
                    const Status status = hypha_waitset_detach(waitset, this->timer->executorId);
                    if (status != HYPHA_STATUS_OK) {
                        hypha::detail::error(
                            "a timer could not detach its registration: status "
                            + std::to_string(hypha::Status(status).asInt())
                            + "; the registration stays until the WaitSet is destroyed"
                        );
                    }
                }
                this->timer->executorId = 0;
            }
        }
        this->timer.reset();
    }

    ffi::TimerPtr timer;
};

inline std::expected<TimerHandle, Status>
createTimer(Duration period, std::function<void()> callback)
{
    std::shared_ptr<ffi::TimerState> held = std::make_shared<ffi::TimerState>();
    held->callback = std::move(callback);

    HyphaWaitSet* waitset = defaultWaitSet();
    if (waitset == nullptr) {
        held->active.store(false, std::memory_order_release);
        return std::unexpected(Status::InvalidState);
    }
    const uint64_t periodNanos = period.nanos() > 0 ? static_cast<uint64_t>(period.nanos()) : 0U;
    held->attachmentRef = held;
    HyphaAttachment record{};
    record.kind = HYPHA_ATTACH_TIMER;
    record.period_nanos = periodNanos;
    record.periodic_callback = &ffi::fireTimer;
    record.context = held.get();
    record.release = &ffi::releaseTimerAttachment;
    const Status status = hypha_waitset_attach(waitset, &record, &held->executorId);
    if (status != HYPHA_STATUS_OK) {
        held->attachmentRef.reset();
        held->active.store(false, std::memory_order_release);
        return std::unexpected(status);
    }
    return TimerHandle(held);
}

}  // namespace hypha::backend
