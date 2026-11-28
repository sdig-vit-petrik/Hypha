#pragma once

/// @file
/// What a worker thread of an ApplicationNode uses where a ROS 1 driver used `ros::ok()`,
/// `ros::Duration::sleep()` and `ros::Rate`: a sleep and a rate that end when the node
/// stops. The stop is the `std::stop_token` of ApplicationNode::stopToken().
///
///     void grabLoop(std::stop_token stop)
///     {
///         hypha::Rate rate(30.0, stop);
///         do {
///             grabAndPublish();
///         } while (rate.sleep());   // false: the node stopped; leave the loop
///     }

#include "hypha_base/time.h"

#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstdint>
#include <mutex>
#include <stdexcept>
#include <stop_token>
#include <type_traits>
#include <utility>

namespace hypha {

namespace detail {

/// The longest span a sleep waits: a sleep for more is a sleep for this. About 100 years,
/// which no steady clock reaches and its time points can still add.
inline constexpr std::chrono::hours kLongestSleep{24 * 365 * 100};

/// The longest period of a Rate. A longer one is no rate a worker keeps a grid for.
inline constexpr std::chrono::hours kLongestPeriod{24};

}  // namespace detail

/// @brief Sleep until @p deadline on the steady clock, or until @p stop is requested.
///
/// Time is host time, not framework time: under simulated time the sleep still takes
/// real time.
/// @return True when the deadline came and no stop was requested; false when the stop
///         ended the sleep, or was there before it. A stopped sleep returns at once every
///         time, so a loop that ignores the result spins: end the loop on false.
[[nodiscard]] inline bool
sleepUntil(std::stop_token stop, std::chrono::steady_clock::time_point deadline)
{
    std::mutex                   mutex;
    std::condition_variable_any  wake;
    std::unique_lock<std::mutex> lock(mutex);
    const auto                   never = [] {
        return false;
    };
    wake.wait_until(lock, stop, deadline, never);
    return !stop.stop_requested();
}

/// @brief Sleep for @p span on the steady clock, or until @p stop is requested.
///
/// A span of zero or less is over at once. A span beyond about a century waits a century.
/// @return As sleepUntil(): true when the whole span passed, false at the stop.
/// @throws std::invalid_argument for a NaN or infinite span of a floating-point duration.
template <typename Rep, typename Period>
[[nodiscard]] bool sleepFor(std::stop_token stop, std::chrono::duration<Rep, Period> span)
{
    if constexpr (std::is_floating_point_v<Rep>) {
        if (!std::isfinite(span.count())) {
            throw std::invalid_argument("hypha::sleepFor: the span is not finite");
        }
    }
    if (span <= span.zero()) {
        return !stop.stop_requested();
    }
    const std::chrono::steady_clock::time_point now = std::chrono::steady_clock::now();
    if (span >= detail::kLongestSleep) {
        return sleepUntil(std::move(stop), now + detail::kLongestSleep);
    }
    const auto whole = std::chrono::ceil<std::chrono::steady_clock::duration>(span);
    return sleepUntil(std::move(stop), now + whole);
}

/// @brief sleepFor() of a hypha::Duration, which `Duration::fromSeconds()` makes from a
///        ROS-style double.
[[nodiscard]] inline bool sleepFor(std::stop_token stop, Duration span)
{
    return sleepFor(std::move(stop), span.chrono());
}

/// @brief A loop's rate on the steady clock, ended by the node's stop: `ros::Rate` for a
///        worker thread.
///
/// The ticks are a grid, the first one period after construction: a pass of work shortens
/// the wait, it does not move the grid. A pass that overran a tick returns from the next
/// sleep() at once; one that overran whole periods skips them, counts them in
/// missedCount() and goes on at the next tick, with no burst to catch up. Not
/// thread-safe: one worker owns it. Time is host time, as for sleepUntil().
class Rate
{
  public:
    /// @param hz The ticks per second.
    /// @param stop Ends the rate: the node's, from ApplicationNode::stopToken().
    /// @throws std::invalid_argument when @p hz is not a finite number above zero, or its
    ///         period is under 1 ns or over 24 h.
    Rate(double hz, std::stop_token stop) : Rate(periodOf(hz), std::move(stop))
    {
    }

    /// @param period The time between ticks, 1 ns to 24 h.
    /// @throws std::invalid_argument for a period outside that range.
    Rate(Duration period, std::stop_token stop)
        : stop_(std::move(stop)), period_(period.chrono()), next_{}
    {
        if (period_ <= period_.zero() || period_ > detail::kLongestPeriod) {
            throw std::invalid_argument("hypha::Rate: the period is outside 1 ns to 24 h");
        }
        this->reset();
    }

    /// @brief Wait for the next tick, or for the stop.
    /// @return True after the tick, also when it was already past; false when the stop
    ///         ended the wait or was there before it, whatever the tick: leave the loop.
    [[nodiscard]] bool sleep()
    {
        const std::chrono::steady_clock::time_point now = std::chrono::steady_clock::now();
        if (now - this->next_ >= this->period_) {
            const auto skipped = (now - this->next_) / this->period_;
            this->next_ += skipped * this->period_;
            this->missed_ += static_cast<uint64_t>(skipped);
        }
        if (!sleepUntil(this->stop_, this->next_)) {
            return false;
        }
        this->next_ += this->period_;
        return true;
    }

    /// @brief Start the grid again: the next tick is one period from now. The count of
    ///        missed periods stays.
    void reset()
    {
        this->next_ = std::chrono::steady_clock::now() + this->period_;
    }

    Duration period() const
    {
        return Duration::fromNanos(this->period_.count());
    }

    /// @return The ticks sleep() skipped because a pass of the loop ran longer than a
    ///         period, in all.
    uint64_t missedCount() const
    {
        return this->missed_;
    }

  private:
    static Duration periodOf(double hz)
    {
        const bool usable = std::isfinite(hz) && hz > 0.0;
        if (!usable) {
            throw std::invalid_argument("hypha::Rate: the rate is not a finite number above zero");
        }
        // Past the range of a Duration the period is out of range too; the constructor says so.
        return Duration::fromSeconds(1.0 / hz);
    }

    std::stop_token                       stop_;
    std::chrono::nanoseconds              period_;
    std::chrono::steady_clock::time_point next_;
    uint64_t                              missed_{0};
};

}  // namespace hypha
