#include "test_types.h"

#include <geometry_msgs/Pose.h>
#include <gtest/gtest.h>
#include <rosgraph_msgs/Clock.h>
#include <sensor_msgs/Image.h>
#include <std_msgs/Empty.h>
#include <std_msgs/String.h>

#include <optional>

using hypha::Publisher;
using hypha_test::Reading;
using hypha_test::testTopic;

namespace {

class PublisherFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("publisher_test");
    }

    std::optional<hypha::Node> node;
};

}  // namespace

TEST_F(PublisherFixture, EmptyTopicYieldsADisabledPublisher)
{
    auto pub = hypha::Publisher<Reading>();
    EXPECT_FALSE(pub.enabled());
    EXPECT_TRUE(pub.topic().empty());
}

TEST_F(PublisherFixture, SubscriberCountFollowsTheListenerLifetime)
{
    auto pub = hypha::Node().advertise<Reading>(hypha::Topic{testTopic("/readings")});
    {
        auto sub = hypha::Node().subscribe<Reading>(hypha::Topic{testTopic("/readings")});
        ASSERT_TRUE(pub.subscriberCount().has_value());
        EXPECT_EQ(pub.subscriberCount().value(), 1U);
    }
    ASSERT_TRUE(pub.subscriberCount().has_value());
    EXPECT_EQ(pub.subscriberCount().value(), 0U);
}

TEST_F(PublisherFixture, EagerPublishIgnoresSubscriberCount)
{
    auto pub = hypha::Node().advertise<Reading>(hypha::Topic{testTopic("/readings")});
    ASSERT_TRUE(pub.subscriberCount().has_value());
    ASSERT_EQ(pub.subscriberCount().value(), 0U);

    EXPECT_TRUE(pub.publish(Reading{1, "a"}));
    EXPECT_EQ(pub.lastPublishStatus(), hypha::Status::Ok);
}

TEST_F(PublisherFixture, DisabledPublisherReportsInvalidState)
{
    auto pub = hypha::Publisher<Reading>();

    EXPECT_FALSE(pub.publish(Reading{1, "a"}));
    EXPECT_EQ(pub.lastPublishStatus(), hypha::Status::InvalidState);
}

TEST_F(PublisherFixture, LatchedPublisherPublishesWithoutSubscriberSoLateSubscriberGetsTheValue)
{
    auto pub = hypha::Node().advertise<Reading>(
        hypha::Topic{testTopic("/latched_readings")}, hypha::latched
    );
    ASSERT_TRUE(pub.enabled());

    ASSERT_TRUE(pub.publish(Reading{42, "answer"}));

    int  seen = 0;
    auto sub = hypha::Node().subscribe<Reading>(
        hypha::Topic{testTopic("/latched_readings")}, [&](const hypha::MessagePtr<Reading>& msg) {
            seen = msg->value;
        }
    );
    (void)node->runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(seen, 42);
}

/// A raw publisher of @p M's identity with the payload limit @p limit.
template <typename M>
std::expected<hypha::RawPublisher, hypha::Status>
rawPublisherOf(const std::string& topic, uint64_t limit)
{
    return hypha::rawPublisher(
        testTopic(topic),
        {.dataType = hypha::codec::dataType<M>(),
         .md5 = hypha::codec::md5Sum<M>(),
         .definition = hypha::codec::definition<M>()},
        hypha::maxPayloadDynamic(limit)
    );
}

// The library bounds a fixed-size type to its exact size from its definition, whatever
// limit was asked, so its pool fits a container's /dev/shm. Seven float64 make a Pose.
TEST_F(PublisherFixture, AFixedSizeTypeIsBoundedToItsEncodedSize)
{
    static_assert(ros::message_traits::IsFixedSize<geometry_msgs::Pose>::value);
    // No bound on purpose: the fixed size still bounds the publisher to 56 bytes.
    auto pub = hypha::rawPublisher(
        testTopic("/bounded_raw_pose"),
        {.dataType = hypha::codec::dataType<geometry_msgs::Pose>(),
         .md5 = hypha::codec::md5Sum<geometry_msgs::Pose>(),
         .definition = hypha::codec::definition<geometry_msgs::Pose>()}
    );
    ASSERT_TRUE(pub.has_value());
    EXPECT_TRUE(pub->loan(56).has_value());
    EXPECT_EQ(pub->loan(57).error(), hypha::Status::TooLarge);
}

// A bound has no effect on a fixed-size type, also one below its size: a Clock is 8 bytes.
TEST_F(PublisherFixture, ALimitBelowTheFixedSizeHasNoEffect)
{
    auto clock = rawPublisherOf<rosgraph_msgs::Clock>("/short_clock", 7);
    ASSERT_TRUE(clock.has_value());
    EXPECT_TRUE(clock->loan(8).has_value());
    EXPECT_EQ(clock->loan(9).error(), hypha::Status::TooLarge);
    EXPECT_TRUE(rawPublisherOf<std_msgs::Empty>("/empty", 1).has_value());
}

TEST_F(PublisherFixture, AVariableSizeTypeKeepsTheLimitItAsks)
{
    static_assert(!ros::message_traits::IsFixedSize<std_msgs::String>::value);
    auto pub = rawPublisherOf<std_msgs::String>("/bounded_text", 300);
    ASSERT_TRUE(pub.has_value());
    EXPECT_TRUE(pub->loan(300).has_value());
    EXPECT_EQ(pub->loan(301).error(), hypha::Status::TooLarge);
}

// A variable-size type is growable unless its publisher names a limit: an image is no
// exception, and a named limit bounds it.
TEST_F(PublisherFixture, AVariableSizeImagePublisherIsGrowableUnlessItNamesALimit)
{
    EXPECT_NO_THROW(
        static_cast<void>(
            hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{testTopic("/camera")})
        )
    );
    EXPECT_TRUE(
        hypha::Node()
            .advertise<sensor_msgs::Image>(
                hypha::Topic{testTopic("/camera_bounded")}, hypha::maxPayloadDynamic(1U << 20U)
            )
            .enabled()
    );
}

TEST_F(PublisherFixture, AFixedSizeMessageArrivesThroughItsBoundedPublisher)
{
    auto pub = hypha::Node().advertise<geometry_msgs::Pose>(
        hypha::Topic{testTopic("/bounded_pose")}, hypha::latched
    );
    ASSERT_TRUE(pub.enabled());
    geometry_msgs::Pose pose{};
    pose.position.x = 1.5;
    pose.orientation.w = 1.0;
    ASSERT_TRUE(pub.publish(pose));

    double seenX = 0.0;
    auto   sub = hypha::Node().subscribe<geometry_msgs::Pose>(
        hypha::Topic{testTopic("/bounded_pose")},
        [&seenX](const hypha::MessagePtr<geometry_msgs::Pose>& msg) {
            seenX = msg->position.x;
        }
    );
    (void)node->runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(seenX, 1.5);
}
