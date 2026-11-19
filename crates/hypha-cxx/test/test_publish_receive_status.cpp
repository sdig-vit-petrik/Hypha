// A failure must reach the caller as a failure.
// A publish that did not send must not report success.

#include "test_types.h"

#include <gtest/gtest.h>

#include <optional>
#include <string>

using hypha_test::Reading;
using hypha_test::testTopic;
using namespace hypha::literals;

namespace {

class PublishReceiveStatusFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("publish_receive_status");
    }

    std::optional<hypha::Node> node;
};

TEST_F(PublishReceiveStatusFixture, ARefusedRawPublisherCreationReturnsTheStatus)
{
    auto refused = hypha::rawPublisher(
        testTopic("status_raw_identity"),
        {.dataType = "test/Bytes", .md5 = "not-an-md5", .definition = ""}
    );
    ASSERT_FALSE(refused.has_value());
    EXPECT_EQ(refused.error(), hypha::Status::InvalidTopic);

    auto created = hypha::rawPublisher(
        testTopic("status_raw_identity"),
        {.dataType = "test/Bytes", .md5 = "0123456789abcdef0123456789abcdef", .definition = ""}
    );
    ASSERT_TRUE(created.has_value());
    EXPECT_TRUE(created->isValid());
}

TEST_F(PublishReceiveStatusFixture, AMessageOverThePayloadLimitIsRefusedAsTooLarge)
{
    auto publisher = hypha::Node().advertise<Reading>(
        hypha::Topic{testTopic("status_too_large")}, hypha::maxPayloadDynamic(64)
    );

    EXPECT_TRUE(publisher.publish(Reading{1, "a"}));
    EXPECT_FALSE(publisher.publish(Reading{2, std::string(100, 'x')}));
    EXPECT_EQ(publisher.lastPublishStatus(), hypha::Status::TooLarge);
}

TEST_F(PublishReceiveStatusFixture, ARawLoanOverThePayloadLimitIsRefusedAsTooLarge)
{
    auto bounded = hypha::rawPublisher(
        testTopic("status_raw_bounded"),
        {.dataType = "test/Bytes", .md5 = "0123456789abcdef0123456789abcdef", .definition = ""},
        hypha::maxPayloadDynamic(128)
    );
    ASSERT_TRUE(bounded.has_value());
    EXPECT_TRUE(bounded->loan(128).has_value());
    auto over = bounded->loan(129);
    ASSERT_FALSE(over.has_value());
    EXPECT_EQ(over.error(), hypha::Status::TooLarge);

    auto growable = hypha::rawPublisher(
        testTopic("status_raw_growable"),
        {.dataType = "test/Bytes", .md5 = "0123456789abcdef0123456789abcdef", .definition = ""}
    );
    ASSERT_TRUE(growable.has_value());
    EXPECT_TRUE(growable->loan(1024 * 1024).has_value());
}

}  // namespace
