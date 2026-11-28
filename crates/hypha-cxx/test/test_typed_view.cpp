// A subscriber of views reads each sample in place: the fields are those the typed
// subscriber decodes, from the same bytes, and a view kept past its callback stays valid,
// within the loan budget by its slot and past it by a counted copy.
#include "test_types.h"

#include <gtest/gtest.h>
#include <sensor_msgs/Image.h>
#include <sensor_msgs/PointCloud2.h>

#include <cstdint>
#include <cstring>
#include <memory>
#include <optional>
#include <random>
#include <string>
#include <vector>

using hypha_test::testTopic;
using sensor_msgs::Image;
using sensor_msgs::PointCloud2;

namespace {

class TypedViewFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("typed_view_test");
    }

    void deliver()
    {
        (void)this->node->runFor(hypha::Duration::fromMillis(5));
    }

    std::optional<hypha::Node> node;
};

/// @p view's message, its `header.seq` (the publisher's count) cleared.
template <typename M>
M unsequenced(const hypha::View<M>& view)
{
    M message = hypha::toMessage(view);
    message.header.seq = 0;
    return message;
}

Image image(uint32_t width, uint32_t height, uint8_t seed)
{
    Image message;
    message.header.stamp.secs = seed;
    message.header.frame_id = "camera_" + std::to_string(seed);
    message.height = height;
    message.width = width;
    message.encoding = "mono8";
    message.step = width;
    message.data.resize(static_cast<size_t>(width) * height);
    for (size_t i = 0; i < message.data.size(); ++i) {
        message.data[i] = static_cast<uint8_t>(i * 31 + seed);
    }
    return message;
}

PointCloud2 cloud(uint32_t points, std::mt19937& random)
{
    PointCloud2 message;
    message.header.frame_id = std::string(random() % 40, 'f');
    message.height = 1;
    message.width = points;
    message.fields = {
        {.name = "x", .offset = 0, .datatype = sensor_msgs::PointField::FLOAT32, .count = 1},
        {.name = "y", .offset = 4, .datatype = sensor_msgs::PointField::FLOAT32, .count = 1},
        {.name = "ring", .offset = 8, .datatype = sensor_msgs::PointField::UINT16, .count = 1},
    };
    // A multiple of 4: a float field of a message's array is aligned (the iterator reads it
    // through a float reference). A received view's bytes are not aligned: read them with memcpy.
    message.point_step = 12;
    message.row_step = points * 12;
    message.is_bigendian = (random() % 2) == 1;
    message.is_dense = (random() % 2) == 1;
    message.data.resize(static_cast<size_t>(points) * 12);
    for (uint8_t& byte : message.data) {
        byte = static_cast<uint8_t>(random());
    }
    return message;
}

}  // namespace

TEST_F(TypedViewFixture, AViewHasTheFieldsTheTypedSubscriberDecodesFromTheSameFrame)
{
    const std::string        topic = testTopic("/cloud");
    std::vector<PointCloud2> decoded;
    std::vector<PointCloud2> viewed;
    std::vector<bool>        inPlace;
    auto                     typed = hypha::Node().subscribe<PointCloud2>(
        hypha::Topic{topic}, [&decoded](const hypha::MessagePtr<PointCloud2>& message) {
            decoded.push_back(*message);
        }
    );
    auto views = hypha::Node().subscribe<hypha::View<PointCloud2>>(
        hypha::Topic{topic}, [&](const hypha::ViewPtr<PointCloud2>& view) {
            viewed.push_back(hypha::toMessage(*view));
            inPlace.push_back(view.use_count() == 1);
        }
    );
    auto pub = hypha::Node().advertise<PointCloud2>(hypha::Topic{topic});

    std::mt19937 random(255);
    for (int round = 0; round < 40; ++round) {
        const uint32_t points = (round % 4 == 0) ? 0U : static_cast<uint32_t>(random() % 3000);
        ASSERT_TRUE(pub.publish(cloud(points, random)));
        this->deliver();
    }
    ASSERT_EQ(decoded.size(), 40U);
    ASSERT_EQ(viewed, decoded);
}

TEST_F(TypedViewFixture, AViewKeptPastItsCallbackKeepsItsSlotThenACopyPastTheBudget)
{
    const std::string                  topic = testTopic("/image");
    std::vector<hypha::ViewPtr<Image>> kept;
    auto                               views = hypha::Node().subscribe<hypha::View<Image>>(
        hypha::Topic{topic}, [&kept](const hypha::ViewPtr<Image>& view) {
            kept.push_back(view);
        }
    );
    auto pub = hypha::Node().advertise<Image>(hypha::Topic{topic});

    ASSERT_TRUE(pub.publish(image(64, 48, 1)));
    this->deliver();
    ASSERT_EQ(kept.size(), 1U);
    EXPECT_EQ(views.metrics().loanHeld, 1U) << "the first view keeps its slot";
    EXPECT_EQ(views.metrics().loanBudget, 1U);

    ASSERT_TRUE(pub.publish(image(64, 48, 2)));
    ASSERT_TRUE(pub.publish(image(32, 16, 3)));
    this->deliver();
    ASSERT_EQ(kept.size(), 3U);
    EXPECT_EQ(views.metrics().loanHeld, 1U) << "past the budget the bytes are copied";
    EXPECT_EQ(views.metrics().loanRefused, 2U) << "and each copy is counted";
    for (uint8_t seed = 1; seed <= 3; ++seed) {
        const Image expected = seed == 3 ? image(32, 16, 3) : image(64, 48, seed);
        EXPECT_EQ(unsequenced(*kept[seed - 1U]), expected) << "frame " << int(seed);
    }

    kept.clear();
    EXPECT_EQ(views.metrics().loanHeld, 0U) << "the last copy returns the slot";
    ASSERT_TRUE(pub.publish(image(8, 8, 4)));
    this->deliver();
    EXPECT_EQ(views.metrics().loanHeld, 1U) << "a free budget keeps the next view in place";
    kept.clear();
}

TEST_F(TypedViewFixture, ReplacingTheKeptViewInTheCallbackKeepsEveryFrameInPlace)
{
    const std::string     topic = testTopic("/image");
    hypha::ViewPtr<Image> latest;
    std::vector<Image>    seen;
    auto                  views = hypha::Node().subscribe<hypha::View<Image>>(
        hypha::Topic{topic}, [&](const hypha::ViewPtr<Image>& view) {
            latest = view;
            seen.push_back(hypha::toMessage(*latest));
        }
    );
    auto pub = hypha::Node().advertise<Image>(hypha::Topic{topic});
    for (uint8_t seed = 1; seed <= 5; ++seed) {
        ASSERT_TRUE(pub.publish(image(16, 4, seed)));
        this->deliver();
        EXPECT_EQ(views.metrics().loanHeld, 1U);
    }
    EXPECT_EQ(views.metrics().loanRefused, 0U) << "the ROS pattern `latest_ = msg` copies nothing";
    EXPECT_EQ(unsequenced(*latest), image(16, 4, 5));
    EXPECT_EQ(seen.size(), 5U);
}

TEST(TypedView, AnEmptyViewPtrIsFalseAndAViewOfAMessageOutlivesTheCaller)
{
    hypha::ViewPtr<Image> empty;
    EXPECT_FALSE(empty);
    EXPECT_TRUE(empty == nullptr);
    hypha::ViewPtr<Image> view;
    {
        auto message = std::make_shared<Image>(image(4, 4, 9));
        view = message;
    }
    EXPECT_EQ(hypha::toMessage(*view), image(4, 4, 9)) << "the view keeps the message";
}
