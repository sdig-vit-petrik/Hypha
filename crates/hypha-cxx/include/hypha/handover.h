#pragma once

/// @file
/// The hand-over from a thread the node does not own to the thread that owns its publishers.
/// A vendor SDK calls back on its own threads (a lidar's packet, cloud and IMU callbacks); a
/// `Publisher` belongs to one thread, so those callbacks must not publish. They push into a
/// Handover and one worker of the application pops and publishes: ROS 1's callback queue,
/// bounded, with a counter on what it dropped.
///
///     // The vendor's callbacks, on its threads:
///     void onCloud(const Cloud& cloud) { handover_.push(toRos(cloud)); }
///     void onImu(const Imu& imu)       { handover_.push(toRos(imu)); }
///
///     // The worker, the one user of the publishers (started in started(), joined in the
///     // destructor); handover_ was built with this->stopToken():
///     while (auto message = handover_.pop()) {
///         std::visit([this](const auto& content) { this->send(content); }, *message);
///     }
///
/// Each kind of message has its own depth, so a burst of small messages never pushes a point
/// cloud out. A kind at its depth drops its oldest message and counts it: dropped(), and a
/// warning in the log at most once a second. Nothing is dropped silently, and a push never
/// waits.

#include "hypha/detail/report.h"

#include <array>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <stop_token>
#include <string>
#include <type_traits>
#include <utility>
#include <variant>

namespace hypha {

namespace detail {

/// True when no type of the pack is there twice.
template <typename... Ts>
struct Distinct : std::true_type
{
};

template <typename T, typename... Rest>
struct Distinct<T, Rest...>
    : std::bool_constant<(!std::is_same_v<T, Rest> && ...) && Distinct<Rest...>::value>
{
};

/// A message was dropped at its depth: a warning at most once a second, the counts attached.
inline void warnHandoverDropped(std::size_t kind, uint64_t droppedInAll)
{
#if !defined(HYPHA_NO_LOGGING)
    HYPHA_LOG_WARN_THROTTLE(
        1.0,
        "hand-over full: dropped the oldest message of kind {} ({} dropped so far)",
        kind,
        droppedInAll
    );
#else
    if (droppedInAll == 1U) {
        warn("hand-over full: dropped the oldest message of kind " + std::to_string(kind));
    }
#endif
}

}  // namespace detail

/// @brief A bounded queue of messages of several kinds, from any number of threads to one.
///
/// @tparam Kinds The message types it carries, each at most once: one queue per kind, so
///         one kind at its depth never displaces another. `pop()` returns them in the order
///         they were pushed, as a `std::variant<Kinds...>`.
///
/// Thread-safe: `push()` from any thread, `pop()` from the one worker. Messages of one
/// pushing thread arrive in the order that thread pushed them. The stop that ends a waiting
/// `pop()` is the token given at construction, normally `ApplicationNode::stopToken()`.
template <typename... Kinds>
class Handover
{
    static_assert(sizeof...(Kinds) > 0U, "a Handover carries at least one kind of message");
    static_assert(detail::Distinct<Kinds...>::value, "a Handover carries each kind once");

  public:
    using Item = std::variant<Kinds...>;

    /// The number of kinds.
    static constexpr std::size_t kKinds = sizeof...(Kinds);

    /// @brief Every kind has @p depth messages.
    /// @param stop Ends the waiting of `pop()` and `popFor()`: the node's, from
    ///        `ApplicationNode::stopToken()`. A default token never stops them.
    /// @throws std::invalid_argument for a depth of zero.
    Handover(std::size_t depth, std::stop_token stop) : Handover(filled(depth), std::move(stop))
    {
    }

    /// @brief Each kind has its own depth, in the order of @p Kinds: `Handover<Cloud, Imu>
    ///        queue({2, 400}, stop)` holds two clouds and 400 IMU samples.
    /// @throws std::invalid_argument for a depth of zero.
    Handover(const std::array<std::size_t, kKinds>& depths, std::stop_token stop)
        : depths_(depths), stop_(std::move(stop))
    {
        for (const std::size_t depth : depths) {
            if (depth == 0U) {
                throw std::invalid_argument("hypha::Handover: a depth must be at least 1");
            }
        }
    }

    Handover(const Handover&) = delete;
    Handover& operator=(const Handover&) = delete;

    /// @brief Hand over one message. Any thread; never waits.
    ///
    /// A kind at its depth loses its oldest message, which is counted in `dropped()` and
    /// warned of in the log. A push after the stop is kept and counted like any other.
    /// @return True when the oldest message of its kind was dropped to make room.
    bool push(Item item)
    {
        const std::size_t kind = item.index();
        bool              displaced = false;
        uint64_t          droppedInAll = 0U;
        {
            const std::lock_guard<std::mutex> lock(this->mutex_);
            std::deque<Entry>&                queue = this->queues_[kind];
            displaced = queue.size() >= this->depths_[kind];
            if (displaced) {
                queue.pop_front();
                ++this->dropped_[kind];
                droppedInAll = this->droppedLocked();
            }
            queue.push_back(Entry{this->arrived_++, std::move(item)});
        }
        this->wake_.notify_one();
        if (displaced) {
            detail::warnHandoverDropped(kind, droppedInAll);
        }
        return displaced;
    }

    /// @brief Take the oldest message, waiting until there is one. The worker.
    /// @return Empty only after the stop, and only once everything queued has been
    ///         returned: a stop drains the queue, then ends the loop.
    std::optional<Item> pop()
    {
        std::unique_lock<std::mutex> lock(this->mutex_);
        this->wake_.wait(lock, this->stop_, [this] {
            return this->queuedLocked();
        });
        return this->takeLocked();
    }

    /// @brief Take the oldest message, waiting at most @p timeout. The worker.
    /// @return Empty when the time ran out (the stop is not requested: `stopped()` is
    ///         false) and after the stop once everything queued has been returned.
    template <typename Rep, typename Period>
    std::optional<Item> popFor(std::chrono::duration<Rep, Period> timeout)
    {
        std::unique_lock<std::mutex> lock(this->mutex_);
        this->wake_.wait_for(lock, this->stop_, timeout, [this] {
            return this->queuedLocked();
        });
        return this->takeLocked();
    }

    /// @brief True once the stop of the token is requested.
    bool stopped() const
    {
        return this->stop_.stop_requested();
    }

    /// @brief The messages dropped at their depth since construction, all kinds together.
    uint64_t dropped() const
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        return this->droppedLocked();
    }

    /// @brief The messages of one kind dropped at its depth since construction.
    template <typename Kind>
    uint64_t dropped() const
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        return this->dropped_[indexOf<Kind>()];
    }

    /// @brief The messages queued now, all kinds together.
    std::size_t pending() const
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        std::size_t                       count = 0U;
        for (const std::deque<Entry>& queue : this->queues_) {
            count += queue.size();
        }
        return count;
    }

  private:
    struct Entry
    {
        uint64_t order;
        Item     item;
    };

    static std::array<std::size_t, kKinds> filled(std::size_t depth)
    {
        std::array<std::size_t, kKinds> depths{};
        depths.fill(depth);
        return depths;
    }

    template <typename Kind>
    static constexpr std::size_t indexOf()
    {
        constexpr std::array<bool, kKinds> same{std::is_same_v<Kind, Kinds>...};
        static_assert((... || std::is_same_v<Kind, Kinds>), "not a kind of this Handover");
        std::size_t index = 0U;
        while (!same[index]) {
            ++index;
        }
        return index;
    }

    bool queuedLocked() const
    {
        for (const std::deque<Entry>& queue : this->queues_) {
            if (!queue.empty()) {
                return true;
            }
        }
        return false;
    }

    uint64_t droppedLocked() const
    {
        uint64_t total = 0U;
        for (const uint64_t count : this->dropped_) {
            total += count;
        }
        return total;
    }

    /// The front message that arrived first, over all kinds.
    std::optional<Item> takeLocked()
    {
        std::deque<Entry>* oldest = nullptr;
        for (std::deque<Entry>& queue : this->queues_) {
            if (!queue.empty()
                && (oldest == nullptr || queue.front().order < oldest->front().order)) {
                oldest = &queue;
            }
        }
        if (oldest == nullptr) {
            return std::nullopt;
        }
        std::optional<Item> item(std::move(oldest->front().item));
        oldest->pop_front();
        return item;
    }

    std::array<std::size_t, kKinds>       depths_;
    std::stop_token                       stop_;
    mutable std::mutex                    mutex_;
    std::condition_variable_any           wake_;
    std::array<std::deque<Entry>, kKinds> queues_;
    std::array<uint64_t, kKinds>          dropped_{};
    uint64_t                              arrived_ = 0U;
};

}  // namespace hypha
