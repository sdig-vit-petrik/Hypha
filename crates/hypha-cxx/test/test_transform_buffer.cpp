// TransformBuffer and StaticTransformBroadcaster: TF in a plain node, on the real bus.
// Host time; the stamps of the transforms are data.

#include "hypha/transform_buffer.h"

#include "test_types.h"

#include <gtest/gtest.h>

#include <atomic>
#include <chrono>
#include <cstdint>
#include <expected>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

using hypha_test::testTopic;
using namespace hypha::literals;

namespace {

using Found = std::expected<geometry_msgs::TransformStamped, hypha::TransformError>;

hypha::TransformTopics topics()
{
    return hypha::TransformTopics{.tf = testTopic("/tf"), .tfStatic = testTopic("/tf_static")};
}

geometry_msgs::TransformStamped
transform(const std::string& parent, const std::string& child, uint32_t seconds, double x)
{
    geometry_msgs::TransformStamped item{};
    item.header.frame_id = parent;
    item.header.stamp.secs = seconds;
    item.child_frame_id = child;
    item.transform.translation.x = x;
    item.transform.rotation.w = 1.0;
    return item;
}

tf2_msgs::TFMessage message(const geometry_msgs::TransformStamped& item)
{
    tf2_msgs::TFMessage result{};
    result.transforms.push_back(item);
    return result;
}

/// Stores each TFMessage a subscriber delivers.
struct KeepMessages
{
    std::vector<tf2_msgs::TFMessage>* received;

    void operator()(const hypha::MessagePtr<tf2_msgs::TFMessage>& delivered) const
    {
        this->received->push_back(*delivered);
    }
};

/// Looks up at a time until it finds a transform, or the deadline passes.
struct LookUpFromAThread
{
    const hypha::TransformBuffer* buffer;
    std::atomic<bool>*            finished;
    std::atomic<bool>*            found;

    void operator()() const
    {
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
        while (std::chrono::steady_clock::now() < deadline) {
            const Found answer = this->buffer->lookup("map", "base", hypha::Time::fromSeconds(2.0));
            if (answer.has_value()) {
                this->found->store(true);
                break;
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
        this->finished->store(true);
    }
};

/// The dynamic transform map -> base at 1 s (x = 0) and 3 s (x = 2), in the buffer.
void feedTwoSamples(hypha::Node& node, hypha::TransformBuffer& buffer)
{
    auto publisher = node.advertise<tf2_msgs::TFMessage>(hypha::Topic{topics().tf});
    ASSERT_TRUE(publisher.publish(message(transform("map", "base", 1, 0.0))));
    ASSERT_TRUE(publisher.publish(message(transform("map", "base", 3, 2.0))));
    ASSERT_TRUE(hypha_test::runUntil(node, [&buffer] {
        const Found latest = buffer.lookupLatest("map", "base");
        return latest.has_value() && latest->header.stamp.secs == 3U;
    }));
}

}  // namespace

TEST(TransformBuffer, ALookupBetweenTwoDynamicTransformsInterpolates)
{
    hypha::Node            node("tfb_interpolates");
    hypha::TransformBuffer buffer{node, topics()};
    feedTwoSamples(node, buffer);

    const Found found = buffer.lookup("map", "base", hypha::Time::fromSeconds(2.0));
    ASSERT_TRUE(found.has_value());
    EXPECT_EQ(found->header.frame_id, "map");
    EXPECT_EQ(found->child_frame_id, "base");
    EXPECT_EQ(found->header.stamp.secs, 2U);
    EXPECT_NEAR(found->transform.translation.x, 1.0, 1e-9);
    EXPECT_EQ(found->transform.rotation.w, 1.0);
}

TEST(TransformBuffer, LookupLatestGivesTheNewestTimeOfTheChain)
{
    hypha::Node            node("tfb_latest");
    hypha::TransformBuffer buffer{node, topics()};
    feedTwoSamples(node, buffer);

    const Found latest = buffer.lookupLatest("map", "base");
    ASSERT_TRUE(latest.has_value());
    EXPECT_EQ(latest->header.stamp.secs, 3U);
    EXPECT_NEAR(latest->transform.translation.x, 2.0, 1e-9);

    const Found inverse = buffer.lookupLatest("base", "map");
    ASSERT_TRUE(inverse.has_value());
    EXPECT_NEAR(inverse->transform.translation.x, -2.0, 1e-9);
}

TEST(TransformBuffer, AStaticTransformSentBeforeTheBufferExistedIsFoundAfterTheNodeRan)
{
    hypha::Node node("tfb_static_late");
    auto        statics =
        node.advertise<tf2_msgs::TFMessage>(hypha::Topic{topics().tfStatic}, hypha::latched);
    ASSERT_TRUE(statics.publish(message(transform("base", "lidar", 5, 0.25))));

    hypha::TransformBuffer buffer{node, topics()};
    ASSERT_TRUE(hypha_test::runUntil(node, [&buffer] {
        return buffer.lookupLatest("base", "lidar").has_value();
    }));
    const Found found = buffer.lookupLatest("base", "lidar");
    ASSERT_TRUE(found.has_value());
    EXPECT_EQ(found->transform.translation.x, 0.25);
    EXPECT_EQ(found->header.stamp.secs, 0U) << "an all-static chain holds at time 0";
}

TEST(TransformBuffer, ALookupThatFindsNothingNamesTheReason)
{
    hypha::Node            node("tfb_errors");
    hypha::TransformBuffer buffer{node, topics()};
    feedTwoSamples(node, buffer);
    auto other = node.advertise<tf2_msgs::TFMessage>(hypha::Topic{topics().tf});
    ASSERT_TRUE(other.publish(message(transform("world", "tool", 2, 1.0))));
    ASSERT_TRUE(hypha_test::runUntil(node, [&buffer] {
        return buffer.lookupLatest("world", "tool").has_value();
    }));

    const hypha::Time inside = hypha::Time::fromSeconds(2.0);
    const Found       empty = buffer.lookup("", "base", inside);
    ASSERT_FALSE(empty.has_value());
    EXPECT_EQ(empty.error(), hypha::TransformError::InvalidFrame);
    const Found slash = buffer.lookup("map", "/", inside);
    ASSERT_FALSE(slash.has_value());
    EXPECT_EQ(slash.error(), hypha::TransformError::InvalidFrame);

    const Found unknown = buffer.lookup("map", "nowhere", inside);
    ASSERT_FALSE(unknown.has_value());
    EXPECT_EQ(unknown.error(), hypha::TransformError::UnknownFrame);

    const Found apart = buffer.lookupLatest("map", "tool");
    ASSERT_FALSE(apart.has_value());
    EXPECT_EQ(apart.error(), hypha::TransformError::NotConnected);

    const Found before = buffer.lookup("map", "base", hypha::Time::fromSeconds(0.5));
    ASSERT_FALSE(before.has_value());
    EXPECT_EQ(before.error(), hypha::TransformError::BeforeBuffer);

    const Found after = buffer.lookup("map", "base", hypha::Time::fromSeconds(9.0));
    ASSERT_FALSE(after.has_value());
    EXPECT_EQ(after.error(), hypha::TransformError::AfterBuffer);

    const Found leading = buffer.lookup("/map", "/base", inside);
    EXPECT_TRUE(leading.has_value()) << "one leading slash is dropped";
    const Found same = buffer.lookup("nowhere", "nowhere", inside);
    EXPECT_TRUE(same.has_value()) << "equal names give the identity";
}

TEST(TransformBuffer, AnotherThreadLooksUpWhileTheNodeRuns)
{
    hypha::Node            node("tfb_thread");
    hypha::TransformBuffer buffer{node, topics()};
    auto publisher = node.advertise<tf2_msgs::TFMessage>(hypha::Topic{topics().tf});
    ASSERT_TRUE(publisher.publish(message(transform("map", "base", 1, 0.0))));
    ASSERT_TRUE(publisher.publish(message(transform("map", "base", 3, 2.0))));

    std::atomic<bool> finished{false};
    std::atomic<bool> found{false};
    std::thread       worker{LookUpFromAThread{&buffer, &finished, &found}};

    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    while (!finished.load() && std::chrono::steady_clock::now() < deadline) {
        ASSERT_TRUE(node.runFor(1_ms).ok());
    }
    worker.join();
    EXPECT_TRUE(finished.load()) << "the worker did not finish within its bound";
    EXPECT_TRUE(found.load());
}

TEST(TransformBuffer, ABufferOfAnotherPolicyOnAnExistingTopicIsRefused)
{
    hypha::Node node("tfb_policy");
    auto first = node.advertise<tf2_msgs::TFMessage>(hypha::Topic{topics().tf}, hypha::defaultQos);
    EXPECT_THROW(hypha::TransformBuffer(node, topics()), hypha::ConfigError);
}

TEST(TransformBuffer, ACacheThatIsNotPositiveIsRefusedBeforeTheCall)
{
    hypha::Node       node("tfb_cache");
    const std::string text = hypha_test::refusalOf([&node] {
        hypha::TransformBuffer buffer{node, topics(), 0_s};
    });
    EXPECT_NE(text.find("InvalidArgument"), std::string::npos) << text;
    const std::string negative = hypha_test::refusalOf([&node] {
        hypha::TransformBuffer buffer{node, topics(), hypha::Duration::fromMillis(-5)};
    });
    EXPECT_NE(negative.find("InvalidArgument"), std::string::npos) << negative;
}

TEST(TransformBuffer, ABufferDestroyedAfterItsNodeDoesNotCrash)
{
    std::unique_ptr<hypha::Node>            node = std::make_unique<hypha::Node>("tfb_after_node");
    std::unique_ptr<hypha::TransformBuffer> buffer =
        std::make_unique<hypha::TransformBuffer>(*node, topics());
    ASSERT_TRUE(node->runFor(2_ms).ok());
    node.reset();
    buffer.reset();
    SUCCEED();
}

TEST(TransformBuffer, AMovedBufferKeepsLookingUp)
{
    hypha::Node            node("tfb_move");
    hypha::TransformBuffer first{node, topics()};
    feedTwoSamples(node, first);
    hypha::TransformBuffer second{std::move(first)};
    EXPECT_TRUE(second.lookup("map", "base", hypha::Time::fromSeconds(2.0)).has_value());
    EXPECT_THROW(
        static_cast<void>(first.lookup("map", "base", hypha::Time::fromSeconds(2.0))),
        std::runtime_error
    );
}

TEST(StaticTransformBroadcaster, TwoSendsReachALateBuffer)
{
    hypha::Node                       node("stb_two_sends");
    hypha::StaticTransformBroadcaster statics{node, hypha::Topic{topics().tfStatic}};
    EXPECT_TRUE(statics.send(transform("map", "odom", 0, 1.0)));
    EXPECT_TRUE(statics.send(transform("base", "lidar", 0, 0.5)));

    hypha::TransformBuffer buffer{node, topics()};
    ASSERT_TRUE(hypha_test::runUntil(node, [&buffer] {
        return buffer.lookupLatest("map", "odom").has_value()
            && buffer.lookupLatest("base", "lidar").has_value();
    }));
    EXPECT_EQ(buffer.lookupLatest("map", "odom")->transform.translation.x, 1.0);
    EXPECT_EQ(buffer.lookupLatest("base", "lidar")->transform.translation.x, 0.5);
}

TEST(StaticTransformBroadcaster, EachSendPublishesTheWholeSetInOneMessage)
{
    hypha::Node                       node("stb_whole_set");
    hypha::StaticTransformBroadcaster statics{node, hypha::Topic{topics().tfStatic}};
    ASSERT_TRUE(statics.send(transform("map", "odom", 0, 1.0)));
    ASSERT_TRUE(statics.send(transform("base", "lidar", 0, 0.5)));

    std::vector<tf2_msgs::TFMessage> received;
    auto                             late = node.subscribe<tf2_msgs::TFMessage>(
        hypha::Topic{topics().tfStatic}, KeepMessages{&received}
    );
    ASSERT_TRUE(hypha_test::runUntil(node, [&received] {
        return !received.empty();
    }));
    ASSERT_EQ(received.size(), 1U) << "a late subscriber gets the one latched message";
    ASSERT_EQ(received.front().transforms.size(), 2U);
    EXPECT_EQ(received.front().transforms[0].child_frame_id, "odom");
    EXPECT_EQ(received.front().transforms[1].child_frame_id, "lidar");
}

TEST(StaticTransformBroadcaster, ALaterTransformOfAChildFrameReplacesTheEarlierOne)
{
    hypha::Node                       node("stb_replace");
    hypha::StaticTransformBroadcaster statics{node, hypha::Topic{topics().tfStatic}};
    ASSERT_TRUE(statics.send(transform("map", "odom", 0, 1.0)));
    ASSERT_TRUE(statics.send(transform("base", "lidar", 0, 0.5)));
    ASSERT_TRUE(statics.send(transform("map", "/odom", 0, 3.0)));

    ASSERT_EQ(statics.transforms().size(), 2U);
    EXPECT_EQ(statics.transforms()[0].transform.translation.x, 3.0) << "replaced in place";
    EXPECT_EQ(statics.transforms()[1].child_frame_id, "lidar");

    std::vector<tf2_msgs::TFMessage> received;
    auto                             late = node.subscribe<tf2_msgs::TFMessage>(
        hypha::Topic{topics().tfStatic}, KeepMessages{&received}
    );
    ASSERT_TRUE(hypha_test::runUntil(node, [&received] {
        return !received.empty();
    }));
    ASSERT_EQ(received.back().transforms.size(), 2U);
    EXPECT_EQ(received.back().transforms[0].transform.translation.x, 3.0);
}

TEST(StaticTransformBroadcaster, AVectorOfTransformsIsOneMessage)
{
    hypha::Node                       node("stb_vector");
    hypha::StaticTransformBroadcaster statics{node, hypha::Topic{topics().tfStatic}};
    const std::vector<geometry_msgs::TransformStamped> batch{
        transform("map", "odom", 0, 1.0),
        transform("base", "lidar", 0, 0.5),
        transform("base", "camera", 0, 0.7),
    };
    EXPECT_TRUE(statics.send(batch));
    EXPECT_EQ(statics.lastPublishStatus(), hypha::Status::Ok);
    EXPECT_TRUE(statics.send(std::vector<geometry_msgs::TransformStamped>{}));
    EXPECT_EQ(statics.transforms().size(), 3U);

    std::vector<tf2_msgs::TFMessage> received;
    auto                             late = node.subscribe<tf2_msgs::TFMessage>(
        hypha::Topic{topics().tfStatic}, KeepMessages{&received}
    );
    ASSERT_TRUE(hypha_test::runUntil(node, [&received] {
        return !received.empty();
    }));
    ASSERT_EQ(received.size(), 1U);
    EXPECT_EQ(received.front().transforms.size(), 3U);
}
