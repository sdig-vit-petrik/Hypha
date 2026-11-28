// The hand-over from a thread the node does not own to the thread that owns its publishers
// (#233): a bounded queue that drops its oldest message, counts every drop, and ends at the
// node's stop. Every wait here is bounded; a hand-over that hangs fails the test, it does not
// hang it.

#include "hypha/handover.h"
#include "hypha/hypha.h"

#include <gtest/gtest.h>

#include <array>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <memory>
#include <optional>
#include <stdexcept>
#include <stop_token>
#include <string>
#include <thread>
#include <utility>
#include <variant>
#include <vector>

namespace {

using Clock = std::chrono::steady_clock;
using std::chrono::milliseconds;

int64_t elapsedMillis(Clock::time_point start)
{
    return std::chrono::duration_cast<milliseconds>(Clock::now() - start).count();
}

/// Two kinds, as an IMU sample and a point cloud are two: an int and a string.
using Pair = hypha::Handover<int, std::string>;

/// The messages out of @p queue until it is empty, as the numbers of the int kind and the
/// texts of the string kind in the order of arrival.
std::vector<std::string> drain(Pair& queue)
{
    std::vector<std::string> out;
    while (std::optional<Pair::Item> item = queue.popFor(milliseconds(0))) {
        if (const int* number = std::get_if<int>(&*item)) {
            out.push_back(std::to_string(*number));
        } else {
            out.push_back(std::get<std::string>(*item));
        }
    }
    return out;
}

TEST(Handover, ADepthOfZeroIsRefused)
{
    EXPECT_THROW(Pair(0U, std::stop_token{}), std::invalid_argument);
    EXPECT_THROW((Pair({2U, 0U}, std::stop_token{})), std::invalid_argument);
}

TEST(Handover, ItDeliversInTheOrderOfArrivalAcrossKinds)
{
    Pair queue(4U, std::stop_token{});
    EXPECT_FALSE(queue.push(1));
    EXPECT_FALSE(queue.push(std::string("a")));
    EXPECT_FALSE(queue.push(2));
    EXPECT_FALSE(queue.push(std::string("b")));
    EXPECT_EQ(drain(queue), (std::vector<std::string>{"1", "a", "2", "b"}));
    EXPECT_EQ(queue.dropped(), 0U);
}

TEST(Handover, AFullKindDropsItsOldestAndCountsIt)
{
    Pair queue(2U, std::stop_token{});
    EXPECT_FALSE(queue.push(1));
    EXPECT_FALSE(queue.push(2));
    EXPECT_TRUE(queue.push(3));  // the depth of the int kind is reached: 1 is dropped
    EXPECT_TRUE(queue.push(4));  // 2 is dropped
    EXPECT_EQ(queue.dropped(), 2U);
    EXPECT_EQ(queue.dropped<int>(), 2U);
    EXPECT_EQ(queue.dropped<std::string>(), 0U);
    EXPECT_EQ(drain(queue), (std::vector<std::string>{"3", "4"}));
}

TEST(Handover, ABurstOfOneKindNeverPushesAnotherOut)
{
    Pair queue(2U, std::stop_token{});
    EXPECT_FALSE(queue.push(std::string("cloud")));
    for (int sample = 0; sample < 100; ++sample) {
        (void)queue.push(sample);
    }
    EXPECT_EQ(queue.dropped<int>(), 98U);
    EXPECT_EQ(queue.dropped<std::string>(), 0U);
    EXPECT_EQ(queue.dropped(), 98U);
    EXPECT_EQ(drain(queue), (std::vector<std::string>{"cloud", "98", "99"}));
}

TEST(Handover, EachKindHasItsOwnDepth)
{
    Pair queue(std::array<std::size_t, 2>{3U, 1U}, std::stop_token{});
    for (int sample = 0; sample < 4; ++sample) {
        (void)queue.push(sample);
    }
    (void)queue.push(std::string("old"));
    (void)queue.push(std::string("new"));
    EXPECT_EQ(queue.dropped<int>(), 1U);
    EXPECT_EQ(queue.dropped<std::string>(), 1U);
    EXPECT_EQ(drain(queue), (std::vector<std::string>{"1", "2", "3", "new"}));
}

TEST(Handover, ItCarriesAKindThatCanOnlyBeMoved)
{
    hypha::Handover<std::unique_ptr<int>> queue(1U, std::stop_token{});
    EXPECT_FALSE(queue.push(std::make_unique<int>(7)));
    EXPECT_TRUE(queue.push(std::make_unique<int>(8)));
    std::optional<std::variant<std::unique_ptr<int>>> item = queue.popFor(milliseconds(0));
    ASSERT_TRUE(item.has_value());
    EXPECT_EQ(*std::get<0>(*item), 8);
    EXPECT_EQ(queue.dropped(), 1U);
}

TEST(Handover, ManyThreadsPushAndEveryMessageIsDeliveredOrCounted)
{
    constexpr int kThreads = 8;
    constexpr int kPerThread = 2000;
    for (const std::size_t depth : {std::size_t{1}, std::size_t{16}, std::size_t{100000}}) {
        std::stop_source              stop;
        hypha::Handover<int>          queue(depth, stop.get_token());
        std::atomic<int64_t>          delivered{0};
        std::vector<std::atomic<int>> last(kThreads);
        std::atomic<bool>             outOfOrder{false};
        std::thread                   consumer([&] {
            while (auto item = queue.pop()) {
                const int value = std::get<0>(*item);
                const int from = value / kPerThread;
                const int seen = last[static_cast<std::size_t>(from)].exchange(value);
                // One thread's messages arrive in its order, whatever the others do.
                if (seen > value) {
                    outOfOrder = true;
                }
                ++delivered;
            }
        });
        std::vector<std::thread>      producers;
        for (int thread = 0; thread < kThreads; ++thread) {
            producers.emplace_back([&queue, thread] {
                for (int index = 0; index < kPerThread; ++index) {
                    (void)queue.push(thread * kPerThread + index);
                }
            });
        }
        for (std::thread& producer : producers) {
            producer.join();
        }
        stop.request_stop();
        consumer.join();  // drains what is queued, then ends
        EXPECT_FALSE(outOfOrder.load()) << "depth " << depth;
        EXPECT_EQ(delivered.load() + static_cast<int64_t>(queue.dropped()), kThreads * kPerThread)
            << "depth " << depth;
        if (depth == 100000U) {
            EXPECT_EQ(queue.dropped(), 0U);
        }
    }
}

TEST(Handover, APopReceivesThePushOfAnotherThread)
{
    Pair                      queue(2U, std::stop_token{});
    std::optional<Pair::Item> received;
    std::thread               consumer([&] {
        received = queue.pop();
    });
    (void)queue.push(std::string("late"));
    consumer.join();
    ASSERT_TRUE(received.has_value());
    EXPECT_EQ(std::get<std::string>(*received), "late");
}

TEST(Handover, PopForReturnsEmptyAtTheTimeoutAndTheStopIsNotThere)
{
    std::stop_source        stop;
    Pair                    queue(2U, stop.get_token());
    const Clock::time_point start = Clock::now();
    EXPECT_FALSE(queue.popFor(milliseconds(100)).has_value());
    EXPECT_GE(elapsedMillis(start), 90);
    EXPECT_FALSE(queue.stopped());
}

TEST(Handover, TheStopEndsAPopAndALongPopForThatWait)
{
    std::stop_source          stop;
    Pair                      queue(2U, stop.get_token());
    std::atomic<bool>         returned{false};
    std::optional<Pair::Item> result = Pair::Item{0};
    std::optional<Pair::Item> timed = Pair::Item{0};
    std::thread               waiting([&] {
        result = queue.pop();
        returned = true;
    });
    std::thread               timing([&] {
        timed = queue.popFor(std::chrono::seconds(30));
    });
    // Time for both to wait: the stop is the stimulus. A pop that returned without a push
    // or a stop is found below; one that ends without the stop hangs the test.
    std::this_thread::sleep_for(milliseconds(50));
    EXPECT_FALSE(returned.load());
    stop.request_stop();
    waiting.join();
    timing.join();
    EXPECT_FALSE(result.has_value());
    EXPECT_FALSE(timed.has_value());
    EXPECT_TRUE(queue.stopped());
}

TEST(Handover, AStopThatWasThereBeforeTheWaitEndsItAtOnce)
{
    std::stop_source stop;
    stop.request_stop();
    Pair queue(2U, stop.get_token());
    EXPECT_FALSE(queue.pop().has_value());
    EXPECT_FALSE(queue.popFor(std::chrono::seconds(30)).has_value());
}

TEST(Handover, TheStopDrainsWhatIsQueuedThenEnds)
{
    std::stop_source stop;
    Pair             queue(4U, stop.get_token());
    (void)queue.push(1);
    (void)queue.push(std::string("a"));
    (void)queue.push(2);
    stop.request_stop();
    // Both pops return what is queued, however they wait, before they end.
    std::optional<Pair::Item> first = queue.pop();
    ASSERT_TRUE(first.has_value());
    EXPECT_EQ(std::get<int>(*first), 1);
    std::optional<Pair::Item> second = queue.popFor(std::chrono::seconds(30));
    ASSERT_TRUE(second.has_value());
    EXPECT_EQ(std::get<std::string>(*second), "a");
    EXPECT_TRUE(queue.pop().has_value());
    EXPECT_FALSE(queue.pop().has_value());
    EXPECT_FALSE(queue.pop().has_value());
}

TEST(Handover, APushAfterTheStopIsKeptAndCountedLikeAnyOther)
{
    std::stop_source stop;
    stop.request_stop();
    Pair queue(1U, stop.get_token());
    EXPECT_FALSE(queue.push(1));
    EXPECT_TRUE(queue.push(2));
    EXPECT_EQ(queue.dropped(), 1U);
    EXPECT_EQ(drain(queue), (std::vector<std::string>{"2"}));
}

/// The vendor SDK's thread pushes; the worker pops. The node ends when the worker has seen
/// enough: the node's stop wakes nothing the worker still waits on.
struct HandoverResult
{
    std::atomic<int>      received{0};
    std::atomic<bool>     endedAtStop{false};
    std::atomic<uint64_t> dropped{0};

    void reset()
    {
        this->received = 0;
        this->endedAtStop = false;
        this->dropped = 0;
    }
};

HandoverResult gResult;

class HandoverApp final : public hypha::ApplicationNode
{
  public:
    HandoverApp() : queue_(8U, this->stopToken())
    {
    }

    HandoverApp(const HandoverApp&) = delete;
    HandoverApp& operator=(const HandoverApp&) = delete;

    ~HandoverApp() override
    {
        // The stop is requested by now: both threads end, the worker after it drained.
        if (this->sdk_.joinable()) {
            this->sdk_.join();
        }
        if (this->worker_.joinable()) {
            this->worker_.join();
        }
        gResult.dropped = this->queue_.dropped();
    }

    void started(hypha::Node&) override
    {
        this->worker_ = std::thread([this] {
            this->serve();
        });
        this->sdk_ = std::thread([this] {
            for (int sample = 0; !this->stopToken().stop_requested(); ++sample) {
                (void)this->queue_.push(sample);
                std::this_thread::yield();
            }
        });
    }

  private:
    void serve()
    {
        while (std::optional<hypha::Handover<int>::Item> item = this->queue_.pop()) {
            if (++gResult.received == 20) {
                hypha::requestStop();
            }
        }
        gResult.endedAtStop = this->queue_.stopped();
    }

    hypha::Handover<int> queue_;
    std::thread          worker_;
    std::thread          sdk_;
};

TEST(Handover, ANodeEndsWhileItsWorkerWaitsOnTheHandover)
{
    gResult.reset();
    EXPECT_TRUE(hypha::disableSimTime());
    char      program[] = "prog";
    char*     argv[] = {program, nullptr};
    const int code = hypha::runNode<HandoverApp>(1, argv, "handover_app");
    EXPECT_EQ(code, 0);
    EXPECT_GE(gResult.received.load(), 20);
    EXPECT_TRUE(gResult.endedAtStop.load());
}

}  // namespace
