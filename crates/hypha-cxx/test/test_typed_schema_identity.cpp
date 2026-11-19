// A typed read must check what the sender said the payload is. The schema hash in the frame
// header is the only identity that crosses the wire, so every typed receive path must
// compare it, not only the callback path.

#include "test_types.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <string>

using hypha_test::testTopic;

namespace hypha_test {

/// Two types with one layout and two identities.
struct AlphaCount
{
    int32_t value{0};
};

struct BetaCount
{
    int32_t value{0};
};

}  // namespace hypha_test

namespace ros::message_traits {

template <>
struct DataType<hypha_test::AlphaCount>
{
    static const char* value()
    {
        return "hypha_test/AlphaCount";
    }

    static const char* value(const hypha_test::AlphaCount&)
    {
        return value();
    }
};

template <>
struct MD5Sum<hypha_test::AlphaCount>
{
    static const char* value()
    {
        return "b3087778e93fcd34cc8d65bc54e850d1";
    }

    static const char* value(const hypha_test::AlphaCount&)
    {
        return value();
    }
};

template <>
struct Definition<hypha_test::AlphaCount>
{
    static const char* value()
    {
        return "int32 value\n";
    }

    static const char* value(const hypha_test::AlphaCount&)
    {
        return value();
    }
};

template <>
struct DataType<hypha_test::BetaCount>
{
    static const char* value()
    {
        return "hypha_test/BetaCount";
    }

    static const char* value(const hypha_test::BetaCount&)
    {
        return value();
    }
};

template <>
struct MD5Sum<hypha_test::BetaCount>
{
    static const char* value()
    {
        return "602d642babe509c7c59f497c23e716a9";
    }

    static const char* value(const hypha_test::BetaCount&)
    {
        return value();
    }
};

template <>
struct Definition<hypha_test::BetaCount>
{
    static const char* value()
    {
        return "int32 count\n";
    }

    static const char* value(const hypha_test::BetaCount&)
    {
        return value();
    }
};

}  // namespace ros::message_traits

namespace ros::serialization {

template <>
struct Serializer<hypha_test::AlphaCount>
{
    template <typename Stream, typename T>
    inline static void allInOne(Stream& stream, T m)
    {
        stream.next(m.value);
    }

    ROS_DECLARE_ALLINONE_SERIALIZER
};

template <>
struct Serializer<hypha_test::BetaCount>
{
    template <typename Stream, typename T>
    inline static void allInOne(Stream& stream, T m)
    {
        stream.next(m.value);
    }

    ROS_DECLARE_ALLINONE_SERIALIZER
};

}  // namespace ros::serialization

namespace {

using hypha_test::AlphaCount;
using hypha_test::BetaCount;

class SchemaIdentityFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("schema_identity");
    }

    std::optional<hypha::Node> node;
};

struct BetaCollector
{
    int* count;

    void operator()(const hypha::MessagePtr<BetaCount>&) const
    {
        *this->count += 1;
    }
};

TEST_F(SchemaIdentityFixture, CallbackRefusesAnotherTypeWithTheSameLayout)
{
    const std::string topic = testTopic("identity_callback");
    auto              publisher = hypha::Node().advertise<AlphaCount>(hypha::Topic{topic});

    int                          received = 0;
    hypha::Subscriber<BetaCount> subscriber =
        hypha::Node().subscribe<BetaCount>(hypha::Topic{topic}, BetaCollector{&received});

    ASSERT_TRUE(publisher.publish(AlphaCount{7}));
    (void)node->runFor(hypha::Duration::fromMillis(1));

    EXPECT_EQ(received, 0) << "a foreign type reached the typed callback";
}

TEST_F(SchemaIdentityFixture, CallbackAcceptsTheDeclaredType)
{
    const std::string topic = testTopic("identity_callback_ok");
    auto              publisher = hypha::Node().advertise<BetaCount>(hypha::Topic{topic});

    int                          received = 0;
    hypha::Subscriber<BetaCount> subscriber =
        hypha::Node().subscribe<BetaCount>(hypha::Topic{topic}, BetaCollector{&received});

    ASSERT_TRUE(publisher.publish(BetaCount{7}));
    (void)node->runFor(hypha::Duration::fromMillis(1));

    EXPECT_EQ(received, 1);
}

TEST_F(SchemaIdentityFixture, AZeroSchemaHashDoesNotBypassTheCallbackCheck)
{
    const std::string topic = testTopic("identity_zero_callback");
    // An empty MD5 makes the publisher stamp a zero schema hash. The payload
    // still decodes as BetaCount, so only the identity rule can refuse it.
    auto publisher = hypha::rawPublisher(
                         topic,
                         {.dataType = "hypha_test/BetaCount",
                          .md5 = "00000000000000000000000000000000",
                          .definition = ""}
    )
                         .value();
    ASSERT_TRUE(publisher.isValid());

    int                          received = 0;
    hypha::Subscriber<BetaCount> subscriber =
        hypha::Node().subscribe<BetaCount>(hypha::Topic{topic}, BetaCollector{&received});

    const std::vector<uint8_t> payload{7U, 0U, 0U, 0U};
    EXPECT_EQ(publisher.publish(payload), hypha::Status::Ok);
    (void)node->runFor(hypha::Duration::fromMillis(1));

    EXPECT_EQ(received, 0) << "an unidentified frame reached the typed callback";
}

}  // namespace
