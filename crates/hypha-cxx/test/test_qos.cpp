// The service policy of a typed endpoint: QosOf<M>, the Qos option of one port, the
// queue depth a reader asks for, and the payload bound of a publisher.

#include "hypha/cycle_node.h"
#include "hypha/cycle_transforms.h"
#include "hypha/hypha.h"
#include "hypha/raw_api.h"

#include "test_types.h"

#include <geometry_msgs/Pose.h>
#include <gtest/gtest.h>
#include <std_msgs/ColorRGBA.h>
#include <std_msgs/Float64.h>
#include <std_msgs/String.h>
#include <std_msgs/UInt8MultiArray.h>
#include <tf2_msgs/TFMessage.h>

#include <cstdint>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

using Float64 = std_msgs::Float64;
using hypha_test::refusalOf;
using hypha_test::testTopic;

namespace qos_test {

/// The policy this file maps std_msgs/ColorRGBA to. No other test uses the type.
inline constexpr hypha::Qos COLOUR{
    .name = "test_colour", .publishers = 2, .subscribers = 3, .maxDepth = 4
};

/// The policy this file maps std_msgs/UInt8MultiArray to: its messages are bounded.
inline constexpr hypha::Qos BOUNDED_BYTES{
    .name = "test_bytes", .publishers = 2, .subscribers = 2, .maxDepth = 2, .maxPayloadDynamic = 256
};

/// A policy no type maps to: a port names it as an option.
inline constexpr hypha::Qos OTHER{
    .name = "test_other", .publishers = 3, .subscribers = 3, .maxDepth = 3
};

}  // namespace qos_test

template <>
struct hypha::QosOf<std_msgs::ColorRGBA>
{
    static constexpr hypha::Qos value = qos_test::COLOUR;
};

template <>
struct hypha::QosOf<std_msgs::UInt8MultiArray>
{
    static constexpr hypha::Qos value = qos_test::BOUNDED_BYTES;
};

namespace {

/// The limits of hypha's built-in `default` policy with one of them changed.
constexpr hypha::Qos almostDefault(uint32_t publishers, uint32_t subscribers, uint32_t maxDepth)
{
    return hypha::Qos{
        .name = "default",
        .publishers = publishers,
        .subscribers = subscribers,
        .maxDepth = maxDepth
    };
}

struct NoStep
{
    void operator()(const hypha::CycleTime&) const
    {
    }
};

/// Keeps the size of each text it receives.
struct TextSizes
{
    std::vector<size_t>* sizes;

    void operator()(const hypha::MessagePtr<std_msgs::String>& message) const
    {
        this->sizes->push_back(message->data.size());
    }
};

std_msgs::UInt8MultiArray bytesOfSize(size_t size)
{
    std_msgs::UInt8MultiArray message{};
    message.data.assign(size, 0x5A);
    return message;
}

class QosFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("qos_test");
    }

    void TearDown() override
    {
        this->node.reset();
        static_cast<void>(hypha::disableSimTime());
    }

    std::optional<hypha::Node> node;
};

// hypha::defaultQos repeats the numbers of the library's `default` policy, which live
// below the C ABI. The library refuses it unless the name and every limit are equal.
TEST_F(QosFixture, DefaultQosEqualsTheDefaultPolicyOfTheLibrary)
{
    const std::string topic = testTopic("/library_default");
    auto              undeclared = hypha::rawPublisher(
        topic,
        {.dataType = hypha::codec::dataType<Float64>(),
         .md5 = hypha::codec::md5Sum<Float64>(),
         .definition = hypha::codec::definition<Float64>()}
    );
    ASSERT_TRUE(undeclared.has_value());

    EXPECT_NO_THROW(
        static_cast<void>(this->node->advertise<Float64>(hypha::Topic{topic}, hypha::defaultQos))
    );
    EXPECT_NO_THROW(static_cast<void>(this->node->subscribe<Float64>(hypha::Topic{topic})));

    // The comparison is live: one limit off in any field is refused.
    for (const hypha::Qos& drifted :
         {almostDefault(9, 16, 32), almostDefault(8, 17, 32), almostDefault(8, 16, 33)}) {
        const std::string refusal = refusalOf([&topic, &drifted, this] {
            static_cast<void>(this->node->advertise<Float64>(hypha::Topic{topic}, drifted));
        });
        EXPECT_NE(refusal.find("QosMismatch"), std::string::npos) << refusal;
    }
}

TEST_F(QosFixture, ATypeWithoutASpecialisationDeclaresTheDefaultPolicy)
{
    static_assert(hypha::QosOf<Float64>::value.name == hypha::defaultQos.name);
    const std::string topic = testTopic("/unmapped_type");
    auto              publisher = this->node->advertise<Float64>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());

    // The topic now carries `default`: the same policy by name opens it, another does not.
    EXPECT_NO_THROW(
        static_cast<void>(
            this->node->subscribe<Float64>(hypha::Topic{topic}, {}, hypha::defaultQos)
        )
    );
    const std::string refusal = refusalOf([&topic, this] {
        static_cast<void>(this->node->subscribe<Float64>(hypha::Topic{topic}, {}, qos_test::OTHER));
    });
    EXPECT_NE(refusal.find("QosMismatch"), std::string::npos) << refusal;
}

TEST_F(QosFixture, ASpecialisationOfQosOfDeclaresItsPolicy)
{
    const std::string topic = testTopic("/mapped_type");
    // A subscriber that declares a policy opens the topic before any publisher.
    auto subscriber = this->node->subscribe<std_msgs::ColorRGBA>(hypha::Topic{topic});
    ASSERT_TRUE(subscriber.enabled());

    // The topic carries the mapped policy, not `default`.
    auto first = this->node->advertise<std_msgs::ColorRGBA>(hypha::Topic{topic}, qos_test::COLOUR);
    ASSERT_TRUE(first.enabled());
    const std::string refusal = refusalOf([&topic, this] {
        static_cast<void>(
            this->node->advertise<std_msgs::ColorRGBA>(hypha::Topic{topic}, hypha::defaultQos)
        );
    });
    EXPECT_NE(refusal.find("QosMismatch"), std::string::npos) << refusal;

    // The policy admits two publishers: the one above and one more.
    auto second = this->node->advertise<std_msgs::ColorRGBA>(hypha::Topic{topic});
    ASSERT_TRUE(second.enabled());
    const std::string full = refusalOf([&topic, this] {
        static_cast<void>(this->node->advertise<std_msgs::ColorRGBA>(hypha::Topic{topic}));
    });
    EXPECT_NE(full.find("ResourceLimit"), std::string::npos) << full;
}

// A topic name carries more than one message type (topic name overloading, #209), but the
// service of a name has one policy: types the application maps to different policies cannot
// share a name, and the second endpoint is refused with QosMismatch.
TEST_F(QosFixture, TwoTypesOfOneNameWithTheSamePolicyShareTheTopic)
{
    const std::string topic = testTopic("/overload_same_policy");
    auto              first = this->node->advertise<Float64>(hypha::Topic{topic});
    auto              second = this->node->advertise<std_msgs::String>(hypha::Topic{topic});
    EXPECT_TRUE(first.enabled());
    EXPECT_TRUE(second.enabled());
}

TEST_F(QosFixture, TwoTypesOfOneNameWithTwoPoliciesAreRefusedWhicheverComesFirst)
{
    const std::string colourFirst = testTopic("/overload_colour_first");
    auto colour = this->node->advertise<std_msgs::ColorRGBA>(hypha::Topic{colourFirst});
    ASSERT_TRUE(colour.enabled());
    const std::string refusedDefault = refusalOf([&colourFirst, this] {
        static_cast<void>(this->node->advertise<Float64>(hypha::Topic{colourFirst}));
    });
    EXPECT_NE(refusedDefault.find("QosMismatch"), std::string::npos) << refusedDefault;

    const std::string defaultFirst = testTopic("/overload_default_first");
    auto              plain = this->node->advertise<Float64>(hypha::Topic{defaultFirst});
    ASSERT_TRUE(plain.enabled());
    const std::string refusedColour = refusalOf([&defaultFirst, this] {
        static_cast<void>(this->node->advertise<std_msgs::ColorRGBA>(hypha::Topic{defaultFirst}));
    });
    EXPECT_NE(refusedColour.find("QosMismatch"), std::string::npos) << refusedColour;
}

TEST_F(QosFixture, ThePolicyOptionOfAPortLetsTwoTypesOfOneNameShareAPolicy)
{
    const std::string topic = testTopic("/overload_option");
    auto              first = this->node->advertise<Float64>(hypha::Topic{topic}, qos_test::OTHER);
    auto second = this->node->advertise<std_msgs::ColorRGBA>(hypha::Topic{topic}, qos_test::OTHER);
    EXPECT_TRUE(first.enabled());
    EXPECT_TRUE(second.enabled());
}

TEST_F(QosFixture, TheQosOptionOfAPortWinsOverThePolicyOfTheType)
{
    const std::string topic = testTopic("/option_wins");
    auto              publisher =
        this->node->advertise<std_msgs::ColorRGBA>(hypha::Topic{topic}, qos_test::OTHER);
    ASSERT_TRUE(publisher.enabled());

    EXPECT_NO_THROW(
        static_cast<void>(
            this->node->subscribe<std_msgs::ColorRGBA>(hypha::Topic{topic}, {}, qos_test::OTHER)
        )
    );
    // Without the option the subscriber declares the policy of the type, which the
    // topic does not carry.
    const std::string refusal = refusalOf([&topic, this] {
        static_cast<void>(this->node->subscribe<std_msgs::ColorRGBA>(hypha::Topic{topic}));
    });
    EXPECT_NE(refusal.find("QosMismatch"), std::string::npos) << refusal;
}

TEST_F(QosFixture, ACyclePortWithAnotherPolicyThanItsTopicFailsStart)
{
    const std::string topic = testTopic("/cycle_mismatch");
    auto              publisher = this->node->advertise<Float64>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());

    hypha::Cycle           agreeing("qos_agreeing_cycle");
    hypha::Latest<Float64> same{agreeing, hypha::Topic{topic}, hypha::unsupervised};
    agreeing.setRate(hypha::Hz(100.0));
    const auto started = agreeing.start(*this->node, NoStep{});
    EXPECT_TRUE(started.has_value()) << started.error();

    hypha::Cycle           differing("qos_differing_cycle");
    hypha::Latest<Float64> other{
        differing, hypha::Topic{topic}, qos_test::OTHER, hypha::unsupervised
    };
    differing.setRate(hypha::Hz(100.0));
    const auto refused = differing.start(*this->node, NoStep{});
    ASSERT_FALSE(refused.has_value());
    EXPECT_NE(refused.error().find("QosMismatch"), std::string::npos) << refused.error();
}

TEST_F(QosFixture, AnArrivalsPerStepAboveTheMaxDepthOfItsPolicyIsRefused)
{
    hypha::Cycle cycle("qos_per_step_cycle");
    // The default policy allows a queue of 32.
    EXPECT_NO_THROW(hypha::Arrivals<Float64>(cycle, "fits", hypha::perStep(32)));
    try {
        hypha::Arrivals<Float64> tooDeep(cycle, "imu", hypha::perStep(33));
        FAIL() << "a perStep above maxDepth was accepted";
    } catch (const std::invalid_argument& refused) {
        const std::string text = refused.what();
        EXPECT_NE(text.find("'imu'"), std::string::npos) << text;
        EXPECT_NE(text.find("33 samples per step"), std::string::npos) << text;
        EXPECT_NE(text.find("'default'"), std::string::npos) << text;
        EXPECT_NE(text.find("maxDepth 32"), std::string::npos) << text;
    }

    // The option of the port decides, in both directions.
    EXPECT_NO_THROW(
        hypha::Arrivals<std_msgs::ColorRGBA>(cycle, "deep", qos_test::COLOUR, hypha::perStep(4))
    );
    EXPECT_THROW(
        hypha::Arrivals<Float64>(cycle, "shallow", qos_test::OTHER, hypha::perStep(4)),
        std::invalid_argument
    );
    // The policy of the type decides without an option: COLOUR allows 4.
    EXPECT_THROW(
        hypha::Arrivals<std_msgs::ColorRGBA>(cycle, "mapped", hypha::perStep(5)),
        std::invalid_argument
    );

    // A port that shares a topic is held to the policy of that topic.
    hypha::Latest<Float64> owner{cycle, "owner", qos_test::OTHER};
    EXPECT_NO_THROW(
        hypha::Arrivals<Float64>(cycle, "view", hypha::sharing(owner), hypha::perStep(3))
    );
    EXPECT_THROW(
        hypha::Arrivals<Float64>(cycle, "wide_view", hypha::sharing(owner), hypha::perStep(4)),
        std::invalid_argument
    );
}

TEST_F(QosFixture, AnArrivalsPortQueuesItsPerStepSamples)
{
    const std::string topic = testTopic("/queued_per_step");
    auto              publisher = this->node->advertise<std_msgs::ColorRGBA>(hypha::Topic{topic});

    struct Count
    {
        hypha::Arrivals<std_msgs::ColorRGBA>* port;
        size_t*                               most;

        void operator()(const hypha::CycleTime&) const
        {
            if (this->port->samples().size() > *this->most) {
                *this->most = this->port->samples().size();
            }
        }
    };

    hypha::Cycle                         cycle("qos_queue_cycle");
    hypha::Arrivals<std_msgs::ColorRGBA> port{
        cycle, hypha::Topic{topic}, hypha::perStep(4), hypha::unsupervised
    };
    cycle.setRate(hypha::Hz(100.0));
    size_t     most = 0;
    const auto started = cycle.start(*this->node, Count{&port, &most});
    ASSERT_TRUE(started.has_value()) << started.error();

    for (int index = 0; index < 4; ++index) {
        ASSERT_TRUE(publisher.publish(std_msgs::ColorRGBA{}));
    }
    for (int round = 0; round < 10 && most == 0; ++round) {
        hypha_test::advance(hypha::Duration::fromMillis(10));
        ASSERT_TRUE(this->node->runFor(hypha::Duration::fromMillis(20)).ok());
    }
    EXPECT_EQ(most, 4U);
}

TEST_F(QosFixture, WithoutABoundAVariableSizeMessageIsGrowable)
{
    const std::string   topic = testTopic("/growable_text");
    std::vector<size_t> sizes;
    auto                subscriber =
        this->node->subscribe<std_msgs::String>(hypha::Topic{topic}, TextSizes{&sizes});
    auto publisher = this->node->advertise<std_msgs::String>(hypha::Topic{topic});

    constexpr size_t LARGE = 4U * 1024U * 1024U;
    std_msgs::String message{};
    message.data.assign(LARGE, 'x');
    ASSERT_TRUE(publisher.publish(message)) << publisher.lastPublishStatus().name();
    ASSERT_TRUE(hypha_test::runUntil(*this->node, [&sizes] {
        return !sizes.empty();
    }));
    EXPECT_EQ(sizes.front(), LARGE);
}

TEST_F(QosFixture, TheMaxPayloadDynamicOfThePolicyBoundsAMessage)
{
    static_assert(!ros::message_traits::IsFixedSize<std_msgs::UInt8MultiArray>::value);
    auto publisher =
        this->node->advertise<std_msgs::UInt8MultiArray>(hypha::Topic{testTopic("/policy_bound")});

    EXPECT_TRUE(publisher.publish(bytesOfSize(100)));
    EXPECT_FALSE(publisher.publish(bytesOfSize(1000)));
    EXPECT_EQ(publisher.lastPublishStatus(), hypha::Status::TooLarge);
}

TEST_F(QosFixture, TheMaxPayloadDynamicOptionWinsOverThePolicy)
{
    const std::string topic = testTopic("/option_bound");
    // A larger bound than the 256 bytes of the policy.
    auto wider = this->node->advertise<std_msgs::UInt8MultiArray>(
        hypha::Topic{topic}, hypha::maxPayloadDynamic(4096)
    );
    EXPECT_TRUE(wider.publish(bytesOfSize(1000)));
    EXPECT_FALSE(wider.publish(bytesOfSize(5000)));
    EXPECT_EQ(wider.lastPublishStatus(), hypha::Status::TooLarge);

    // Zero is growable. The bound is not part of the policy two endpoints agree on, so
    // a second publisher of the topic may differ in it.
    auto growable = this->node->advertise<std_msgs::UInt8MultiArray>(
        hypha::Topic{topic}, hypha::maxPayloadDynamic(0)
    );
    EXPECT_TRUE(growable.publish(bytesOfSize(100000)));
}

TEST_F(QosFixture, ABoundHasNoEffectOnAFixedSizeMessage)
{
    static_assert(ros::message_traits::IsFixedSize<geometry_msgs::Pose>::value);
    // A Pose is 56 bytes. A smaller bound, by option or by policy, has no effect.
    static constexpr hypha::Qos SMALL_BOUND{
        .name = "test_small_bound",
        .publishers = 2,
        .subscribers = 2,
        .maxDepth = 2,
        .maxPayloadDynamic = 8
    };
    const std::string topic = testTopic("/fixed_size");
    auto byPolicy = this->node->advertise<geometry_msgs::Pose>(hypha::Topic{topic}, SMALL_BOUND);
    auto byOption = this->node->advertise<geometry_msgs::Pose>(
        hypha::Topic{topic}, SMALL_BOUND, hypha::maxPayloadDynamic(7)
    );
    EXPECT_TRUE(byPolicy.publish(geometry_msgs::Pose{}));
    EXPECT_TRUE(byOption.publish(geometry_msgs::Pose{}));
}

// `/tf` has more publishers than the default policy admits: every node that broadcasts
// a transform is one. hypha maps tf2_msgs/TFMessage to its own policy `tf`.
TEST_F(QosFixture, ATransformTopicAdmitsMorePublishersThanTheDefaultPolicy)
{
    static_assert(hypha::QosOf<tf2_msgs::TFMessage>::value.name == hypha::tfQos.name);
    const std::string                                  topic = testTopic("/tf");
    std::vector<hypha::Publisher<tf2_msgs::TFMessage>> publishers;
    for (int index = 0; index < 12; ++index) {
        EXPECT_NO_THROW(
            publishers.push_back(this->node->advertise<tf2_msgs::TFMessage>(hypha::Topic{topic}))
        ) << "publisher "
          << index;
    }
    EXPECT_EQ(publishers.size(), 12U);

    const std::string refusal = refusalOf([&topic, this] {
        static_cast<void>(
            this->node->advertise<tf2_msgs::TFMessage>(hypha::Topic{topic}, hypha::defaultQos)
        );
    });
    EXPECT_NE(refusal.find("QosMismatch"), std::string::npos) << refusal;
}

TEST_F(QosFixture, ARawEndpointAdoptsTheTopicUnlessItNamesAPolicy)
{
    const std::string    topic = testTopic("/raw_adopts");
    const hypha::RawType type{
        .dataType = hypha::codec::dataType<std_msgs::ColorRGBA>(),
        .md5 = hypha::codec::md5Sum<std_msgs::ColorRGBA>(),
        .definition = hypha::codec::definition<std_msgs::ColorRGBA>()
    };
    // No topic yet: a subscriber that declares nothing cannot create it.
    auto early = hypha::subscribeRawOfType(topic, type.dataType, [](const hypha::RawSample&) {
    });
    ASSERT_FALSE(early.has_value());
    EXPECT_EQ(early.error(), hypha::Status::NotAvailable);

    auto typed = this->node->advertise<std_msgs::ColorRGBA>(hypha::Topic{topic});
    EXPECT_TRUE(hypha::rawPublisher(topic, type).has_value());
    EXPECT_TRUE(hypha::rawPublisher(topic, type, qos_test::COLOUR).has_value());
    const auto mismatched = hypha::rawPublisher(topic, type, qos_test::OTHER);
    ASSERT_FALSE(mismatched.has_value());
    EXPECT_EQ(mismatched.error(), hypha::Status::QosMismatch);
}

}  // namespace
