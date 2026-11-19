#pragma once

/// @file
/// The per-call-site state of the throttled and transition macros.

#include "hypha/hypha_ffi.h"

#include <atomic>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <limits>

namespace hypha::log::detail {

inline constexpr int64_t NEVER_WRITTEN = std::numeric_limits<int64_t>::min();

inline constexpr int32_t STATE_UNSEEN = -1;

inline constexpr double NANOS_PER_SECOND = 1.0e9;

struct TransitionState
{
    std::atomic<int32_t> previous{STATE_UNSEEN};
    std::atomic<int64_t> lastWrite{NEVER_WRITTEN};
};

/// Count one rejected line. A line at level OFF is an invalid line level.
inline void countRejectedLine(HyphaLogModule module)
{
    hypha_log_write(module, HYPHA_LOG_LEVEL_OFF, nullptr, 0);
}

inline bool isValidPeriod(double periodSeconds)
{
    const bool isUsable = std::isfinite(periodSeconds) && periodSeconds > 0.0;
    return isUsable;
}

/// A period too long for int64 nanoseconds never elapses again.
inline int64_t periodToNanos(double periodSeconds)
{
    const double nanos = periodSeconds * NANOS_PER_SECOND;
    const bool   exceedsRange = nanos >= static_cast<double>(std::numeric_limits<int64_t>::max());
    if (exceedsRange) {
        return std::numeric_limits<int64_t>::max();
    }
    return static_cast<int64_t>(nanos);
}

/// @brief Decide whether a throttled line may write now. An invalid period suppresses the
///        line and counts a rejected line.
inline bool
throttlePermits(HyphaLogModule module, std::atomic<int64_t>& lastWrite, double periodSeconds)
{
    if (!isValidPeriod(periodSeconds)) {
        countRejectedLine(module);
        return false;
    }
    const int64_t now = hypha_clock_steady_now_nanos();
    int64_t       last = lastWrite.load(std::memory_order_relaxed);
    const bool    hasWritten = last != NEVER_WRITTEN;
    if (hasWritten) {
        const bool periodHeld = (now - last) < periodToNanos(periodSeconds);
        if (periodHeld) {
            return false;
        }
    }
    return lastWrite.compare_exchange_strong(last, now, std::memory_order_relaxed);
}

/// Record @p current and return the state the previous call saw, or @p initial.
inline bool recordState(TransitionState& state, bool current, bool initial)
{
    const int32_t seen = state.previous.exchange(current ? 1 : 0, std::memory_order_relaxed);
    if (seen != STATE_UNSEEN) {
        return seen == 1;
    }
    state.lastWrite.store(hypha_clock_steady_now_nanos(), std::memory_order_relaxed);
    return initial;
}

inline bool transitionPermits(TransitionState& state, bool current, bool initial)
{
    const bool wasTrue = recordState(state, current, initial);
    const bool isRisingEdge = current && !wasTrue;
    return isRisingEdge;
}

/// @brief Rising edge, or a held true state once per period.
inline bool transitionThrottlePermits(
    HyphaLogModule module, TransitionState& state, bool current, bool initial, double periodSeconds
)
{
    const bool wasTrue = recordState(state, current, initial);
    if (!isValidPeriod(periodSeconds)) {
        countRejectedLine(module);
        return false;
    }
    if (!current) {
        return false;
    }
    if (!wasTrue) {
        state.lastWrite.store(hypha_clock_steady_now_nanos(), std::memory_order_relaxed);
        return true;
    }
    return throttlePermits(module, state.lastWrite, periodSeconds);
}

}  // namespace hypha::log::detail
