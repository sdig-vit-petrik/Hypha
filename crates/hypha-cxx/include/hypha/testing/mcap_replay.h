#pragma once

/// @file
/// testing::McapReplay: replay a recorded MCAP file into the inputs of a cycle through a
/// testing::StepHarness, one step per deadline, and read a topic of the file as golden outputs.
///
///     hypha::testing::StepHarness harness{node};
///     hypha::testing::McapReplay  replay{"fixture.mcap", hypha::Hz(20.0)};
///     replay.feed(node.position);
///     while (replay.stepDue()) {
///         const hypha::CycleTime time = replay.next(harness);
///         harness.step(node, time);
///     }
///
/// Link hypha::testing_mcap beside hypha::sdk. The file holds ROS 1 messages.
///
/// Timing rules:
/// - A sample's send time is its MCAP log time.
/// - The deadlines are whole multiples of the period since the epoch; the first is the
///   first multiple strictly after the earliest fed sample.
/// - next() delivers every fed sample sent strictly before the deadline, in log-time order:
///   a sample sent exactly at a deadline belongs to the next step. `now` and `stamp` are the
///   deadline, `missed` is 0, `dt` is empty in the first step and one period after.
/// - stepDue() is true while a fed sample remains undelivered; endAt() adds the silent tail.

#include "hypha/backend/ros1_codec.h"
#include "hypha/backend/session.h"
#include "hypha/cycle.h"
#include "hypha/cycle_arrivals.h"
#include "hypha/cycle_input.h"
#include "hypha/cycle_latest.h"
#include "hypha/message_stamp.h"
#include "hypha/testing.h"
#include "hypha/testing/mcap_file.h"
#include "hypha_base/time.h"
#include <hypha/testing/mcap_reader.h>

#include <cstddef>
#include <cstdint>
#include <expected>
#include <limits>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace hypha::testing {

/// @brief One recorded message of a topic, decoded, with its MCAP times.
template <typename M>
struct Recorded
{
    M    message;
    Time logTime;
    Time publishTime;
};

/// @brief Replays the fed topics of one MCAP file into a StepHarness, one step per
///        deadline, and reads any topic of the file as golden outputs.
///
/// Throws std::runtime_error for a fault of the file, std::logic_error for a call out of order.
class McapReplay final
{
  public:
    /// @brief Open the file at @p path for a replay at @p rate, the rate of the cycle.
    /// @throws std::invalid_argument when @p rate gives no period of 1 ns or more.
    /// @throws std::runtime_error when the file does not open (the reader's text).
    McapReplay(const std::string& path, Hz rate)
        : path_(path), periodNanos_(McapReplay::periodOf(path, rate)), file_(path)
    {
    }

    McapReplay(const McapReplay&) = delete;
    McapReplay& operator=(const McapReplay&) = delete;
    McapReplay(McapReplay&&) = delete;
    McapReplay& operator=(McapReplay&&) = delete;

    ~McapReplay()
    {
        for (Feed& feed : this->feeds_) {
            feed.destroy(feed.window);
        }
    }

    /// @brief Replay the topic of @p port into it, the topic start() would read. A disabled
    ///        optional port is never fed.
    /// @throws std::logic_error when the port was fed already, the replay has started
    ///         (stepDue() or next() was called), or a table topic finds no Node.
    /// @throws std::runtime_error when the topic does not resolve, the file holds no such
    ///         topic, or a channel of it is not ROS 1 or was recorded as another type
    ///         name or MD5 than @p M has.
    template <typename M>
    void feed(Latest<M>& port)
    {
        static_cast<void>(
            this->addFeed(port, nullptr, &McapReplay::takeLatest<M>, &McapReplay::keep, true)
        );
    }

    /// @brief Replay the topic of @p port into it; the rules of feed(Latest&).
    template <typename M>
    void feed(Arrivals<M>& port)
    {
        std::unique_ptr<Window<M>> window = std::make_unique<Window<M>>();
        static_cast<void>(this->addFeed(
            port, window.get(), &McapReplay::takeArrival<M>, &McapReplay::flushArrivals<M>, true
        ));
        static_cast<void>(window.release());
    }

    /// @brief feed(), but a topic that the file does not hold leaves the port unfed.
    /// @return True when the port is fed; false when the file holds no such topic or the
    ///         port is disabled.
    /// @throws as feed(), except for a topic the file does not hold.
    template <typename M>
    [[nodiscard]] bool feedIfPresent(Latest<M>& port)
    {
        return this->addFeed(port, nullptr, &McapReplay::takeLatest<M>, &McapReplay::keep, false);
    }

    /// @brief feedIfPresent(Latest&) for an Arrivals port.
    template <typename M>
    [[nodiscard]] bool feedIfPresent(Arrivals<M>& port)
    {
        std::unique_ptr<Window<M>> window = std::make_unique<Window<M>>();
        const bool                 fed = this->addFeed(
            port, window.get(), &McapReplay::takeArrival<M>, &McapReplay::flushArrivals<M>, false
        );
        static_cast<void>(window.release());
        return fed;
    }

    /// @brief Step on after the last fed sample, through the last deadline at or before
    ///        @p end, so the fed ports go stale by their `silence_ms`.
    void endAt(Time end)
    {
        this->end_ = end.nanos();
    }

    /// @brief True while a fed sample remains that no next() has delivered, or a deadline
    ///        at or before endAt() remains. The first call starts the replay; feed() after it
    ///        is refused.
    /// @throws std::runtime_error when the file holds no message on any fed topic.
    bool stepDue()
    {
        this->start();
        return this->due();
    }

    /// @brief Deliver to @p harness every fed sample sent strictly before the next
    ///        deadline, then return the CycleTime of that deadline, for harness.step().
    /// @throws std::logic_error when no step is due (stepDue() is false).
    /// @throws std::runtime_error when a payload does not decode, or a time does not fit
    ///         framework time.
    CycleTime next(StepHarness& harness)
    {
        this->start();
        if (!this->due()) {
            throw std::logic_error(
                "hypha: McapReplay '" + this->path_
                + "': next() with no step due: every fed sample was delivered"
            );
        }
        const int64_t deadline = this->nextDeadline_;
        while (this->hasPending_) {
            const bool sentBeforeDeadline = this->pendingNanos_ < deadline;
            if (!sentBeforeDeadline) {
                break;
            }
            this->deliverPending(harness);
            this->advance();
        }
        for (Feed& feed : this->feeds_) {
            feed.flush(feed, harness);
        }
        CycleTime time;
        time.now = Time::fromNanos(deadline);
        time.stamp = toRosTime(time.now);
        if (this->previousDeadline_.has_value()) {
            time.dt = Duration::fromNanos(deadline - *this->previousDeadline_);
        }
        time.missed = 0;
        this->previousDeadline_ = deadline;
        const bool deadlineOverflows =
            deadline > std::numeric_limits<int64_t>::max() - this->periodNanos_;
        this->lastDeadlineReached_ = deadlineOverflows;
        const bool moreToCome = this->hasPending_ && deadlineOverflows;
        if (moreToCome) {
            throw std::runtime_error(
                "hypha: McapReplay '" + this->path_ + "': the deadline after "
                + std::to_string(deadline) + " ns does not fit framework time"
            );
        }
        if (!deadlineOverflows) {
            this->nextDeadline_ = deadline + this->periodNanos_;
        }
        return time;
    }

    /// @brief Every message of @p topic, decoded, in log-time order. Does not move the replay.
    /// @throws std::runtime_error as feed() for the topic and its channels, and when a
    ///         payload does not decode or a time does not fit framework time.
    template <typename M>
    std::vector<Recorded<M>> read(const std::string& topic) const
    {
        detail::McapFile file(this->path_);
        McapReplay::checkChannels<M>(file, this->path_, topic);
        file.select(topic);
        std::vector<Recorded<M>> recorded;
        HyphaMcapMessage         message{};
        while (file.next(message)) {
            Recorded<M> item{};
            const bool  decoded =
                codec::decode(message.payload.data, message.payload.len, item.message);
            if (!decoded) {
                this->throwUndecodable(topic, message, codec::dataType<M>());
            }
            item.logTime = Time::fromNanos(this->nanosOf(message.log_time_ns));
            item.publishTime = Time::fromNanos(this->nanosOf(message.publish_time_ns));
            recorded.push_back(std::move(item));
        }
        return recorded;
    }

  private:
    template <typename M>
    struct Window
    {
        std::vector<M> samples;
        Time           newestSentAt;
    };

    /// One fed port. `window` (owned through `destroy`) is null for a Latest port. A
    /// disabled port has an empty topic.
    struct Feed
    {
        Port*       port;
        std::string topic;
        void*       window;
        bool (*take)(Feed&, StepHarness&, const HyphaMcapMessage&, Time);
        void (*flush)(Feed&, StepHarness&);
        void (*destroy)(void*);
        const char* typeName;
    };

    using Take = bool (*)(Feed&, StepHarness&, const HyphaMcapMessage&, Time);
    using Flush = void (*)(Feed&, StepHarness&);

    // Defined in hypha/testing/mcap_replay_impl.h.
    static int64_t periodOf(const std::string& path, Hz rate);
    template <typename M>
    bool        addFeed(Input<M>& port, void* window, Take take, Flush flush, bool mustBePresent);
    static bool holdsTopic(const detail::McapFile& file, const std::string& topic);
    bool        due() const;
    void        refuseFeed(const Port& port) const;
    template <typename M>
    std::string topicOf(const Input<M>& port) const;
    template <typename M>
    static void
    checkChannels(const detail::McapFile& file, const std::string& path, const std::string& topic);
    void              start();
    void              advance();
    void              deliverPending(StepHarness& harness);
    int64_t           nanosOf(uint64_t nanos) const;
    [[noreturn]] void throwUndecodable(
        const std::string& topic, const HyphaMcapMessage& message, const char* typeName
    ) const;
    template <typename M>
    static bool
    takeLatest(Feed& feed, StepHarness& harness, const HyphaMcapMessage& message, Time sentAt);
    template <typename M>
    static bool takeArrival(Feed& feed, StepHarness&, const HyphaMcapMessage& message, Time sentAt);
    template <typename M>
    static void flushArrivals(Feed& feed, StepHarness& harness);
    static void keep(Feed&, StepHarness&);
    template <typename M>
    static void destroyWindow(void* window);

    std::string            path_;
    int64_t                periodNanos_;
    detail::McapFile       file_;
    std::vector<Feed>      feeds_;
    bool                   started_{false};
    bool                   hasPending_{false};
    bool                   lastDeadlineReached_{false};
    std::optional<int64_t> end_;
    HyphaMcapMessage       pending_{};
    int64_t                pendingNanos_{0};
    int64_t                nextDeadline_{0};
    std::optional<int64_t> previousDeadline_;
};

}  // namespace hypha::testing

#include "hypha/testing/mcap_replay_impl.h"
