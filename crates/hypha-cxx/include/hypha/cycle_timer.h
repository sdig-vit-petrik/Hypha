#pragma once

/// @file
/// CycleTimer: a second rate inside a step, the ros::Timer of a cycle.

#include "hypha/cycle.h"
#include "hypha_base/time.h"

#include <cstdint>
#include <optional>
#include <stdexcept>

namespace hypha {

/// @brief A timer that a step polls with its own time: a grid of instants one period apart,
///        and the count of them that passed since the previous poll.
///
///     hypha::CycleTimer status{hypha::Hz(10.0)};            // a member, in a 100 Hz cycle
///
///     void step(const hypha::CycleTime& time) override {
///         if (const hypha::CycleTimer::Tick tick = this->status.poll(time)) {
///             publishStatus(tick);                          // first: ROS ran a due timer
///         }                                                 // before the next callback
///         ...                                               // then read the ports
///     }
///
/// It is a value on the step's time, so it ticks the same on host time, under simulated
/// time, in a replay and in a StepHarness, and two replays give the same ticks. The runtime
/// knows nothing of it: it wakes nothing, a tick runs in the first step at or after its
/// instant, and it allocates nothing.
///
/// - The first poll anchors the grid at the step's time and gives no tick; start(time)
///   anchors it at `time`. The grid never drifts.
/// - A poll gives the count of instants in (previous poll, now]: none when the step is
///   early, one on time, more after a long step. The periods beyond the first are missed:
///   Tick::missed() of this poll, missedTotal() since construction. Run the body once (the
///   rule of a hypha timer) or once per period (what a ROS timer did over a slow callback).
/// - stop() ends the ticks until start(); a stopped timer misses nothing.
/// - Time that goes back (a step of the host wall clock; simulated time ends the run instead)
///   anchors the grid at the new time: no tick, no burst, no wait.
class CycleTimer
{
  public:
    /// @brief What one poll found.
    struct Tick
    {
        /// The instants of the grid that passed since the previous poll; 0 is no tick.
        uint64_t periods{0};
        /// The newest of them (ROS `event.current_expected`).
        Time expected;
        /// The time since the step of the previous tick (ROS `current_real - last_real`):
        /// empty for the first tick after the grid was anchored.
        std::optional<Duration> elapsed;

        explicit operator bool() const
        {
            return this->periods != 0;
        }

        /// The periods of this poll beyond the first.
        uint64_t missed() const
        {
            return this->periods > 1 ? this->periods - 1 : 0;
        }
    };

    /// @brief A running timer, anchored at its first poll.
    /// @throws std::invalid_argument for a period that is not positive.
    explicit CycleTimer(Duration period) : period_(checked(period))
    {
    }

    /// @brief A running timer at @p rate, anchored at its first poll.
    /// @throws std::invalid_argument for a rate that gives no positive period.
    explicit CycleTimer(Hz rate) : CycleTimer(detail::periodFromHz(rate))
    {
    }

    /// @brief The instants that passed by @p time, the step's time. Poll it first in the
    ///        step, before the step reads its ports, as ROS ran a due timer before the next
    ///        callback; and in every step, or the count spans the steps that did not poll.
    [[nodiscard]] Tick poll(const CycleTime& time)
    {
        const Time now = time.now;
        if (!this->running_) {
            return {};
        }
        const bool wentBack = this->lastPoll_.has_value() && now < *this->lastPoll_;
        if (!this->next_.has_value() || wentBack) {
            this->anchor(now);
            return {};
        }
        this->lastPoll_ = now;
        if (now < *this->next_) {
            return {};
        }
        const int64_t period = this->period_.nanos();
        const int64_t passed = (now - *this->next_).nanos() / period + 1;
        Tick          tick;
        tick.periods = static_cast<uint64_t>(passed);
        tick.expected =
            *this->next_ + Duration::fromNanos(detail::saturatingMul(passed - 1, period));
        if (this->lastTick_.has_value()) {
            tick.elapsed = now - *this->lastTick_;
        }
        this->missedTotal_ += tick.missed();
        this->lastTick_ = now;
        *this->next_ = tick.expected + this->period_;
        return tick;
    }

    /// @brief Start a stopped timer, anchored at its next poll. A running one is unchanged,
    ///        as by ROS `start()`.
    void start()
    {
        if (!this->running_) {
            this->running_ = true;
            this->clear();
        }
    }

    /// @brief Start a stopped timer, anchored at @p time: its first tick is one period
    ///        later. A running one is unchanged; stop() and start(time) anchor it again.
    void start(const CycleTime& time)
    {
        if (!this->running_) {
            this->running_ = true;
            this->anchor(time.now);
        }
    }

    /// @brief No tick until start().
    void stop()
    {
        this->running_ = false;
        this->clear();
    }

    bool running() const
    {
        return this->running_;
    }

    Duration period() const
    {
        return this->period_;
    }

    /// The next instant of the grid; empty while stopped and before the first poll.
    std::optional<Time> next() const
    {
        return this->next_;
    }

    /// The periods missed since construction: the sum of Tick::missed().
    uint64_t missedTotal() const
    {
        return this->missedTotal_;
    }

  private:
    static Duration checked(Duration period)
    {
        if (period.nanos() <= 0) {
            throw std::invalid_argument("hypha: a CycleTimer period must be positive");
        }
        return period;
    }

    void anchor(Time now)
    {
        this->next_ = now + this->period_;
        this->lastPoll_ = now;
        this->lastTick_.reset();
    }

    void clear()
    {
        this->next_.reset();
        this->lastPoll_.reset();
        this->lastTick_.reset();
    }

    Duration            period_;
    bool                running_{true};
    std::optional<Time> next_;
    std::optional<Time> lastPoll_;
    std::optional<Time> lastTick_;
    uint64_t            missedTotal_{0};
};

}  // namespace hypha
