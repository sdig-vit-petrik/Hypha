#include "test_types.h"

#include <gtest/gtest.h>

#include <atomic>
#include <cstdint>
#include <optional>
#include <thread>
#include <vector>

using hypha_test::Reading;
using hypha_test::testTopic;

namespace {

/// Number of samples the delivery thread publishes. Each one gives the reader
/// a new value to pair with its send time.
constexpr int PAIR_ITERATIONS = 2000;

/// Run in short steps until the subscriber has delivered sample `value`.
bool deliverUntil(hypha::Node& node, hypha::Subscriber<Reading>& subscriber, int value)
{
    constexpr int MAX_STEPS = 500;
    const auto    delivered = [&] {
        const auto sample = subscriber.latestSample();
        return sample.has_value() && sample->message->value >= value;
    };
    for (int step = 0; step < MAX_STEPS; ++step) {
        if (delivered()) {
            return true;
        }
        (void)node.runFor(hypha::Duration::fromMillis(1));
    }
    return delivered();
}

/// The host steady clock just before and just after publish(i) returned.
/// The send time of sample i must fall inside this window.
struct SendWindow
{
    std::atomic<int64_t> before{0};
    std::atomic<int64_t> after{0};
};

class SubscriberPairFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("subscriber_pair_test");
    }

    std::optional<hypha::Node> node;
};

}  // namespace

TEST_F(SubscriberPairFixture, LatestPairIsConsistentUnderConcurrentDelivery)
{
    // A delivery thread publishes numbered samples and runs the node. The test
    // thread reads the newest value and its send time, one call after the
    // other. A delivery between the two calls leaves a value and a send time
    // from different samples. The send time then falls outside the window of
    // the value's sample.
    const std::string topic = testTopic("/readings_pair");
    auto              pub = hypha::Node().advertise<Reading>(hypha::Topic{topic});
    auto              sub = hypha::Node().subscribe<Reading>(hypha::Topic{topic});

    std::vector<SendWindow> windows(PAIR_ITERATIONS + 1);
    std::atomic<bool>       delivering{true};
    std::atomic<bool>       deliveryOk{true};

    std::thread worker([&] {
        for (int i = 1; i <= PAIR_ITERATIONS; ++i) {
            windows[i].before.store(hypha::steadyNow().nanos(), std::memory_order_release);
            EXPECT_TRUE(pub.publish(Reading{i, "pair"}));
            windows[i].after.store(hypha::steadyNow().nanos(), std::memory_order_release);
            if (!deliverUntil(*node, sub, i)) {
                deliveryOk.store(false);
                break;
            }
        }
        delivering.store(false);
    });

    uint64_t pairsChecked = 0;
    uint64_t mismatches = 0;
    while (delivering.load()) {
        const auto sample = sub.latestSample();
        if (!sample.has_value()) {
            continue;
        }
        const SendWindow& window = windows[static_cast<size_t>(sample->message->value)];
        const int64_t     sentNanos = sample->transportSent.nanos();
        const bool        sentInWindow = sentNanos >= window.before.load(std::memory_order_acquire)
                                      && sentNanos <= window.after.load(std::memory_order_acquire);
        ++pairsChecked;
        if (!sentInWindow) {
            ++mismatches;
        }
    }
    worker.join();

    EXPECT_TRUE(deliveryOk.load()) << "a sample was not delivered in time";
    EXPECT_GT(pairsChecked, 0U);
    EXPECT_EQ(mismatches, 0U) << "value and send time came from different samples; pairs checked: "
                              << pairsChecked;
}
