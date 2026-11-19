#include "test_types.h"

#include <gtest/gtest.h>

#include <chrono>
#include <optional>
#include <thread>
#include <vector>

using hypha_test::Reading;
using hypha_test::testTopic;
using namespace hypha::literals;

namespace {

/// Run in short steps until the subscriber holds a usable value. One short run
/// can end before a loaded machine delivers the sample.
template <typename SubscriberT>
void runUntilUsable(hypha::Node& node, SubscriberT& subscriber)
{
    constexpr int MAX_STEPS = 500;
    for (int step = 0; step < MAX_STEPS; ++step) {
        (void)node.runFor(hypha::Duration::fromMillis(1));
        if (subscriber.latestSample().has_value()) {
            return;
        }
    }
}

class SubscriberFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("subscriber_test");
    }

    std::optional<hypha::Node> node;
};

}  // namespace

TEST_F(SubscriberFixture, StartsWithoutAValue)
{
    auto sub = hypha::Node().subscribe<Reading>(hypha::Topic{testTopic("/readings")});

    const auto value = sub.latestSample();
    ASSERT_FALSE(value.has_value());
    EXPECT_EQ(value.error(), hypha::Status::NotAvailable);
}

TEST_F(SubscriberFixture, EmptyTopicIsRefused)
{
    const std::string refusal = hypha_test::refusalOf([] {
        static_cast<void>(hypha::Node().subscribe<Reading>(hypha::Topic{""}));
    });
    EXPECT_NE(refusal.find("InvalidArgument"), std::string::npos) << refusal;
}

TEST_F(SubscriberFixture, DisabledSubscriberIsDisabledAndHasNoValue)
{
    const hypha::Subscriber<Reading> sub;

    EXPECT_FALSE(sub.enabled());
    const auto value = sub.latestSample();
    ASSERT_FALSE(value.has_value());
    EXPECT_EQ(value.error(), hypha::Status::NotAvailable);
}

TEST_F(SubscriberFixture, TypedWildcardSubscriberReceivesFromAMatchingTopic)
{
    const std::string topic = testTopic("/wild_typed/x");
    auto              pub = hypha::Node().advertise<Reading>(hypha::Topic{topic});
    ASSERT_TRUE(pub.enabled());

    // A wildcard follows the topics that exist when it is created, so the
    // publisher is created first. The subscriber gets the same typed callback
    // a literal topic gets.
    auto sub = hypha::Node().subscribe<Reading>(hypha::Topic{testTopic("/wild_typed/*")});
    ASSERT_TRUE(sub.enabled());

    ASSERT_TRUE(pub.publish(Reading{7, "seven"}));
    runUntilUsable(*node, sub);
    const auto value = sub.latestSample();
    ASSERT_TRUE(value.has_value());
    EXPECT_EQ(value->message->value, 7);
    EXPECT_EQ(value->message->label, "seven");
}

TEST_F(SubscriberFixture, TypedWildcardWithAStarInsideASegmentIsRefused)
{
    // A star that is not a whole segment matches nothing, so the pattern is
    // refused as an invalid topic instead of subscribing to nothing.
    ::testing::internal::CaptureStderr();
    const std::string refusal = hypha_test::refusalOf([] {
        static_cast<void>(
            hypha::Node().subscribe<Reading>(hypha::Topic{testTopic("/wild_typed/b*")})
        );
    });
    (void)::testing::internal::GetCapturedStderr();
    EXPECT_NE(refusal.find("InvalidTopic"), std::string::npos) << refusal;
}

TEST_F(SubscriberFixture, AnArrivalMakesTheValueUsable)
{
    auto pub = hypha::Node().advertise<Reading>(hypha::Topic{testTopic("/readings")});
    auto sub = hypha::Node().subscribe<Reading>(hypha::Topic{testTopic("/readings")});

    ASSERT_TRUE(pub.publish(Reading{5, "five"}));
    runUntilUsable(*node, sub);

    const auto value = sub.latestSample();
    ASSERT_TRUE(value.has_value());
    EXPECT_EQ(value->message->value, 5);
    EXPECT_EQ(sub.messageCount(), 1U);
}

TEST_F(SubscriberFixture, NodeSubscribeAndAdvertise)
{
    const std::string topic = testTopic("/node_pub_sub");
    auto              pub = hypha::Node().advertise<Reading>(hypha::Topic{topic});

    struct ObserveReading
    {
        int* seen;

        void operator()(const hypha::Subscriber<Reading>::ConstPtr& msg) const
        {
            *this->seen = msg->value;
        }
    };

    int  seen = 0;
    auto sub = hypha::Node().subscribe<Reading>(hypha::Topic{topic}, ObserveReading{&seen});
    ASSERT_TRUE(sub.enabled());
    EXPECT_TRUE(pub.publish(Reading{42, "forty-two"}));
    EXPECT_EQ(pub.lastPublishStatus(), hypha::Status::Ok);
    runUntilUsable(*node, sub);
    EXPECT_EQ(seen, 42);
    ASSERT_TRUE(sub.latestSample().has_value());
    EXPECT_EQ(sub.latestSample()->message->value, 42);
}

TEST_F(SubscriberFixture, LatestSampleReportsTheSendTimeOfTheNewestSample)
{
    const std::string topic = testTopic("/readings_transport_sent");
    auto              pub = hypha::Node().advertise<Reading>(hypha::Topic{topic});
    auto              sub = hypha::Node().subscribe<Reading>(hypha::Topic{topic});

    EXPECT_EQ(sub.latestSample().error(), hypha::Status::NotAvailable);

    const hypha::Time beforeFirst = hypha::steadyNow();
    ASSERT_TRUE(pub.publish(Reading{1, "first"}));
    runUntilUsable(*node, sub);
    const hypha::Time afterFirst = hypha::steadyNow();
    const auto        first = sub.latestSample();
    ASSERT_TRUE(first.has_value());
    EXPECT_EQ(first->message->value, 1);
    EXPECT_GE(first->transportSent.nanos(), beforeFirst.nanos());
    EXPECT_LE(first->transportSent.nanos(), afterFirst.nanos());

    const hypha::Time beforeSecond = hypha::steadyNow();
    ASSERT_TRUE(pub.publish(Reading{2, "second"}));
    (void)node->runFor(hypha::Duration::fromMillis(1));
    const auto second = sub.latestSample();
    ASSERT_TRUE(second.has_value());
    EXPECT_EQ(second->message->value, 2);
    EXPECT_GE(second->transportSent.nanos(), beforeSecond.nanos());
    EXPECT_GT(second->transportSent.nanos(), first->transportSent.nanos());
}

TEST(SubscriberNoPublisherWarning, TheDefaultDelayIsFiveSeconds)
{
    constexpr uint64_t FIVE_SECONDS = 5'000'000'000ULL;

    EXPECT_EQ(
        hypha::backend::defaultSubscriberQos(8).no_publisher_warning_after_nanos, FIVE_SECONDS
    );
}
