// Subscriber::missingSequences() and Subscriber::metrics(), and the same on RawSubscriber.
// Host time, on the real bus.

#include "hypha/raw_api.h"

#include "test_types.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <string>

using hypha_test::Reading;
using hypha_test::testTopic;
using namespace hypha::literals;

namespace {

/// A queue of two samples: a burst of more than two between two runs overflows it.
constexpr hypha::Qos SHALLOW{
    .name = "test_diag_shallow", .publishers = 2, .subscribers = 2, .maxDepth = 2
};

/// Counts the samples a run delivers.
struct CountSamples
{
    uint64_t* count;

    void operator()(const hypha::MessagePtr<Reading>&) const
    {
        *this->count += 1U;
    }
};

struct CountRawSamples
{
    uint64_t* count;

    void operator()(const hypha::RawSample&) const
    {
        *this->count += 1U;
    }
};

Reading reading(int value)
{
    Reading result;
    result.value = value;
    return result;
}

}  // namespace

TEST(SubscriberDiagnostics, AQueueOverflowRaisesTheMissingSequenceCount)
{
    hypha::Node node("diag_overflow");
    const auto  topic = hypha::Topic{testTopic("/readings")};
    uint64_t    delivered = 0;
    auto        publisher = node.advertise<Reading>(topic, SHALLOW);
    auto        subscriber = node.subscribe<Reading>(topic, CountSamples{&delivered}, SHALLOW);

    // The first accepted frame starts the count: a loss before it is not counted.
    ASSERT_TRUE(publisher.publish(reading(0)));
    ASSERT_TRUE(hypha_test::runUntil(node, [&delivered] {
        return delivered >= 1U;
    }));
    ASSERT_EQ(subscriber.missingSequences(), 0U);

    constexpr int BURST = 10;
    for (int index = 1; index <= BURST; ++index) {
        ASSERT_TRUE(publisher.publish(reading(index)));
    }
    ASSERT_TRUE(hypha_test::runUntil(node, [&delivered] {
        return delivered >= 3U;
    }));
    EXPECT_EQ(subscriber.missingSequences(), 8U) << "ten sent, the queue held the last two";
    EXPECT_EQ(subscriber.metrics().sequenceGap, 0U) << "a lost frame is no refused frame";
}

TEST(SubscriberDiagnostics, AFreshSubscriberHasOnlyALoanBudget)
{
    hypha::Node node("diag_fresh");
    auto        subscriber = node.subscribe<Reading>(hypha::Topic{testTopic("/readings")});

    const hypha::SubscriberMetrics metrics = subscriber.metrics();
    EXPECT_GT(metrics.loanBudget, 0U);
    hypha::SubscriberMetrics expected{};
    expected.loanBudget = metrics.loanBudget;
    EXPECT_EQ(metrics, expected);
    EXPECT_EQ(subscriber.missingSequences(), 0U);
}

TEST(SubscriberDiagnostics, ADisabledSubscriberReadsZeroEverywhere)
{
    const hypha::Subscriber<Reading> subscriber;
    EXPECT_FALSE(subscriber.enabled());
    EXPECT_EQ(subscriber.missingSequences(), 0U);
    EXPECT_EQ(subscriber.metrics(), hypha::SubscriberMetrics{});

    const hypha::RawSubscriber raw;
    EXPECT_EQ(raw.missingSequences(), 0U);
    EXPECT_EQ(raw.metrics(), hypha::SubscriberMetrics{});
}

TEST(SubscriberDiagnostics, ARawSubscriberCountsAQueueOverflowToo)
{
    hypha::Node       node("diag_raw_overflow");
    const std::string topic = testTopic("/readings");
    uint64_t          delivered = 0;
    auto              publisher = node.advertise<Reading>(hypha::Topic{topic}, SHALLOW);
    std::expected<hypha::RawSubscriber, hypha::Status> raw = hypha::subscribeRawOfType(
        topic, hypha::codec::dataType<Reading>(), CountRawSamples{&delivered}, 0, SHALLOW
    );
    ASSERT_TRUE(raw.has_value());

    ASSERT_TRUE(publisher.publish(reading(0)));
    ASSERT_TRUE(hypha_test::runUntil(node, [&delivered] {
        return delivered >= 1U;
    }));
    for (int index = 1; index <= 6; ++index) {
        ASSERT_TRUE(publisher.publish(reading(index)));
    }
    ASSERT_TRUE(hypha_test::runUntil(node, [&delivered] {
        return delivered >= 3U;
    }));
    EXPECT_EQ(raw->missingSequences(), 4U);
    EXPECT_GT(raw->metrics().loanBudget, 0U);
}
