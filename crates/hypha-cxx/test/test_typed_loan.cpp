#include "test_types.h"

#include <gtest/gtest.h>
#include <sensor_msgs/Image.h>
#include <sensor_msgs/PointCloud2.h>

#include <array>
#include <cstddef>
#include <cstring>
#include <optional>
#include <vector>

using hypha_test::testTopic;

namespace {

class TypedLoanFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("typed_loan_test");
    }

    /// The payloads of @p topic's frames, as they arrive, after a short run.
    template <typename M>
    hypha::RawSubscriber collect(const std::string& topic, std::vector<std::vector<uint8_t>>& into)
    {
        return hypha::subscribeRawOfType(
                   topic,
                   hypha::RawType{
                       .dataType = hypha::codec::dataType<M>(), .md5 = hypha::codec::md5Sum<M>()
                   },
                   [&into](const hypha::RawSample& sample) {
                       into.emplace_back(sample.bytes.begin(), sample.bytes.end());
                   },
                   0,
                   hypha::defaultQos
        )
            .value();
    }

    void deliver()
    {
        (void)this->node->runFor(hypha::Duration::fromMillis(5));
    }

    std::optional<hypha::Node> node;
};

sensor_msgs::Image imageHead()
{
    sensor_msgs::Image head;
    head.header.stamp.secs = 7;
    head.header.frame_id = "camera_optical";
    head.height = 3;
    head.width = 5;
    head.encoding = "rgb8";
    head.step = 15;
    return head;
}

std::vector<uint8_t> pixels(size_t count)
{
    std::vector<uint8_t> bytes(count);
    for (size_t i = 0; i < count; ++i) {
        bytes[i] = static_cast<uint8_t>(i * 13 + 1);
    }
    return bytes;
}

sensor_msgs::PointCloud2 cloudHead()
{
    sensor_msgs::PointCloud2 head;
    head.header.frame_id = "lidar";
    head.height = 1;
    head.fields = {
        {.name = "x", .offset = 0, .datatype = sensor_msgs::PointField::FLOAT32, .count = 1},
        {.name = "ring", .offset = 4, .datatype = sensor_msgs::PointField::UINT16, .count = 1},
    };
    head.point_step = 6;
    head.is_dense = true;
    return head;
}

/// @p message encoded as the typed publisher sends it, with `header.seq` @p seq.
template <typename M>
std::vector<uint8_t> wireOf(M message, uint32_t seq)
{
    message.header.seq = seq;
    return hypha::codec::encode(message);
}

}  // namespace

TEST_F(TypedLoanFixture, AnImageWrittenInPlaceSendsTheBytesOfTheTypedPublish)
{
    const std::string                 topic = testTopic("/image");
    std::vector<std::vector<uint8_t>> seen;
    auto                              sub = this->collect<sensor_msgs::Image>(topic, seen);
    auto pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{topic});

    sensor_msgs::Image whole = imageHead();
    whole.data = pixels(45);
    ASSERT_TRUE(pub.publish(whole));

    auto loan = pub.loan(imageHead(), 45);
    ASSERT_TRUE(loan.has_value()) << loan.error().name();
    ASSERT_EQ(loan->data().size(), 45U);
    std::memcpy(loan->data().data(), whole.data.data(), 45);
    ASSERT_TRUE(pub.publish(std::move(*loan)));
    this->deliver();

    ASSERT_EQ(seen.size(), 2U);
    EXPECT_EQ(seen[0], wireOf(whole, 0));
    EXPECT_EQ(seen[1], wireOf(whole, 1));
}

TEST_F(TypedLoanFixture, ACloudShrunkAtThePublishSendsTheBytesOfTheTypedPublish)
{
    const std::string                 topic = testTopic("/cloud");
    std::vector<std::vector<uint8_t>> seen;
    auto                              sub = this->collect<sensor_msgs::PointCloud2>(topic, seen);
    auto pub = hypha::Node().advertise<sensor_msgs::PointCloud2>(hypha::Topic{topic});

    // Room for 10 points; the revolution ends after 7.
    auto loan = pub.loan(cloudHead(), 10 * 6);
    ASSERT_TRUE(loan.has_value()) << loan.error().name();
    const std::vector<uint8_t> points = pixels(7 * 6);
    for (size_t i = 0; i < 7; ++i) {
        std::array<uint8_t, 6> point{};
        std::memcpy(point.data(), points.data() + i * 6, 6);
        loan->put(i, point);
    }
    loan->message().width = 7;
    loan->message().row_step = 7 * 6;
    loan->message().header.stamp.secs = 9;
    ASSERT_TRUE(pub.publish(std::move(*loan), points.size()));
    this->deliver();

    sensor_msgs::PointCloud2 whole = cloudHead();
    whole.width = 7;
    whole.row_step = 7 * 6;
    whole.header.stamp.secs = 9;
    whole.data = points;
    ASSERT_EQ(seen.size(), 1U);
    EXPECT_EQ(seen[0], wireOf(whole, 0));
}

TEST_F(TypedLoanFixture, ATypedSubscriberReceivesALoanedImageUnchanged)
{
    const std::string                 topic = testTopic("/image");
    std::optional<sensor_msgs::Image> received;
    auto                              sub = hypha::Node().subscribe<sensor_msgs::Image>(
        hypha::Topic{topic}, [&received](const hypha::MessagePtr<sensor_msgs::Image>& message) {
            received = *message;
        }
    );
    auto pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{topic});

    auto loan = pub.loan(imageHead(), 45);
    ASSERT_TRUE(loan.has_value());
    const std::vector<uint8_t> bytes = pixels(45);
    std::memcpy(loan->data().data(), bytes.data(), bytes.size());
    ASSERT_TRUE(pub.publish(std::move(*loan)));
    this->deliver();

    sensor_msgs::Image expected = imageHead();
    expected.data = bytes;
    ASSERT_TRUE(received.has_value());
    EXPECT_EQ(*received, expected);
}

TEST_F(TypedLoanFixture, ALoanPastTheLoanCountIsReportedAndADroppedLoanReturnsItsSlotAndNoSequence)
{
    const std::string                 topic = testTopic("/image");
    std::vector<std::vector<uint8_t>> seen;
    auto                              sub = this->collect<sensor_msgs::Image>(topic, seen);
    auto pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{topic});

    // A publisher holds two loans at once.
    auto first = pub.loan(imageHead(), 45);
    auto second = pub.loan(imageHead(), 45);
    ASSERT_TRUE(first.has_value());
    ASSERT_TRUE(second.has_value());
    const auto third = pub.loan(imageHead(), 45);
    ASSERT_FALSE(third.has_value());
    EXPECT_EQ(third.error(), hypha::Status::LoanLimit);
    EXPECT_EQ(pub.lastPublishStatus(), hypha::Status::LoanLimit);

    {
        const auto dropped = std::move(first);
        const auto alsoDropped = std::move(second);
    }
    auto again = pub.loan(imageHead(), 45);
    ASSERT_TRUE(again.has_value()) << again.error().name();
    const std::vector<uint8_t> bytes = pixels(45);
    std::memcpy(again->data().data(), bytes.data(), bytes.size());
    ASSERT_TRUE(pub.publish(std::move(*again)));
    this->deliver();

    sensor_msgs::Image whole = imageHead();
    whole.data = bytes;
    ASSERT_EQ(seen.size(), 1U);
    EXPECT_EQ(seen[0], wireOf(whole, 0));
}

TEST_F(TypedLoanFixture, ALoanIsRefusedForAFilledFieldAndPastThePayloadLimit)
{
    auto pub = hypha::Node().advertise<sensor_msgs::Image>(
        hypha::Topic{testTopic("/image")}, hypha::maxPayloadDynamic(1000)
    );
    sensor_msgs::Image filled = imageHead();
    filled.data = pixels(3);
    const auto refused = pub.loan(filled, 45);
    ASSERT_FALSE(refused.has_value());
    EXPECT_EQ(refused.error(), hypha::Status::InvalidArgument);

    const auto tooLarge = pub.loan(imageHead(), 2000);
    ASSERT_FALSE(tooLarge.has_value());
    EXPECT_EQ(tooLarge.error(), hypha::Status::TooLarge);
}

TEST_F(TypedLoanFixture, AChangedStringOrAnOverlongSizeSendsNothing)
{
    const std::string                 topic = testTopic("/image");
    std::vector<std::vector<uint8_t>> seen;
    auto                              sub = this->collect<sensor_msgs::Image>(topic, seen);
    auto pub = hypha::Node().advertise<sensor_msgs::Image>(hypha::Topic{topic});

    auto renamed = pub.loan(imageHead(), 45);
    ASSERT_TRUE(renamed.has_value());
    renamed->message().header.frame_id = "another_frame";
    EXPECT_FALSE(pub.publish(std::move(*renamed)));
    EXPECT_EQ(pub.lastPublishStatus(), hypha::Status::InvalidArgument);

    auto overlong = pub.loan(imageHead(), 45);
    ASSERT_TRUE(overlong.has_value());
    EXPECT_FALSE(pub.publish(std::move(*overlong), 46));
    EXPECT_EQ(pub.lastPublishStatus(), hypha::Status::InvalidArgument);

    // Both slots came back: two loans are granted again.
    auto first = pub.loan(imageHead(), 45);
    auto second = pub.loan(imageHead(), 45);
    EXPECT_TRUE(first.has_value());
    EXPECT_TRUE(second.has_value());
    this->deliver();
    EXPECT_TRUE(seen.empty());
}

TEST_F(TypedLoanFixture, ADisabledPublisherLendsNothing)
{
    auto       pub = hypha::Publisher<sensor_msgs::Image>();
    const auto loan = pub.loan(imageHead(), 45);
    ASSERT_FALSE(loan.has_value());
    EXPECT_EQ(loan.error(), hypha::Status::InvalidState);
}
