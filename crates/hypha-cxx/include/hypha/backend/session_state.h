#pragma once

/// @file
/// The state one process holds: the node, the subscriptions and the timers.

#include "hypha/backend/handles.h"
#include "hypha/detail/report.h"
#include "hypha/hypha_ffi.h"
#include "hypha/raw.h"
#include "hypha/status.h"

#include <atomic>
#include <cstdint>
#include <functional>
#include <memory>
#include <mutex>
#include <string>

namespace hypha::backend {

namespace ffi {

/// One subscription's state, registered with the executor.
struct Subscription
{
    std::string                                  pattern;
    std::function<void(const hypha::RawSample&)> callback;
    hypha::detail::RecurringFault notAFrame;
    hypha::detail::RecurringFault callbackFailures;
    std::atomic<bool>             active{true};
    uint64_t                      executorId{0};
    /// Declared last, so it is released before the callback it outlives.
    UniqueSubscriber subscriber;
};

using SubscriptionPtr = std::shared_ptr<ffi::Subscription>;

/// One timer, registered with the executor.
struct TimerState
{
    /// The missed periods the WaitSet reported, summed over every call.
    std::atomic<uint64_t>         missed{0};
    std::function<void()>         callback;
    hypha::detail::RecurringFault callbackFailures;
    std::atomic<bool>             active{true};
    uint64_t                      executorId{0};
    /// The registration's own reference, so a refused detach keeps the state alive.
    std::shared_ptr<TimerState> attachmentRef;
};

using TimerPtr = std::shared_ptr<ffi::TimerState>;

/// Everything one process holds. The node's WaitSet owns the cycles.
struct Session
{
    std::mutex   mutex;
    HyphaNode*   node{nullptr};
    HyphaConfig* config{nullptr};
    std::string  name;
    std::string  parameterFile;
    /// Counts the sessions this process has released; a cycle of an earlier one is dead.
    uint64_t generation{0};
    bool     shuttingDown{false};
};

inline Session& session()
{
    static Session state;
    return state;
}

}  // namespace ffi

}  // namespace hypha::backend
