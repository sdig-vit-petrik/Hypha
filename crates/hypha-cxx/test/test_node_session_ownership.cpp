// The session is shared by every Node that joined it and left by the last of them (#97).

#include "hypha/hypha.h"
#include <hypha_msgs/std_msgs/UInt32.h>

#include <gtest/gtest.h>

#include <memory>
#include <optional>

namespace {

TEST(NodeSessionOwnership, aSecondNodeLeavesTheFirstNodesSessionAlive)
{
    hypha::Node first("session_owner");
    {
        hypha::Node second("session_owner");
        EXPECT_EQ(hypha::nodeName(), "session_owner");
    }
    EXPECT_EQ(hypha::nodeName(), "session_owner");
    EXPECT_NE(hypha::backend::nodeHandle(), nullptr);
}

TEST(NodeSessionOwnership, theCreatingNodeReleasesTheSession)
{
    {
        hypha::Node only("session_release");
        EXPECT_EQ(hypha::nodeName(), "session_release");
    }
    EXPECT_EQ(hypha::backend::nodeHandle(), nullptr);
}

TEST(NodeSessionOwnership, aDefaultConstructedNodeLeavesTheSessionAlive)
{
    hypha::Node creator("session_default");
    {
        hypha::Node handle;
    }
    EXPECT_EQ(hypha::nodeName(), "session_default");
    EXPECT_NE(hypha::backend::nodeHandle(), nullptr);
}

TEST(NodeSessionOwnership, theFirstNodeDestroyedFirstLeavesTheSecondOnTheBus)
{
    auto first = std::make_unique<hypha::Node>("session_order");
    {
        hypha::Node second("session_order");
        auto        publisher = second.advertise<std_msgs::UInt32>(hypha::Topic{"/session_order"});
        first.reset();
        EXPECT_TRUE(hypha::ok()) << "the session outlives the Node that created it";
        EXPECT_NE(hypha::backend::nodeHandle(), nullptr);
        EXPECT_TRUE(second.runFor(hypha::Duration::fromMillis(1)).ok());
        std_msgs::UInt32 value{};
        EXPECT_TRUE(publisher.publish(value));
    }
    EXPECT_EQ(hypha::backend::nodeHandle(), nullptr) << "the last Node leaves";
}

TEST(NodeSessionOwnership, theLastNodeDestroyedLastLeavesInEitherOrder)
{
    std::optional<hypha::Node> first;
    first.emplace("session_either");
    {
        hypha::Node second("session_either");
    }
    EXPECT_NE(hypha::backend::nodeHandle(), nullptr) << "the first Node still holds it";
    first.reset();
    EXPECT_EQ(hypha::backend::nodeHandle(), nullptr);
}

TEST(NodeSessionOwnership, aPublisherOfALeftSessionIsRefusedWithInvalidState)
{
    std::optional<hypha::Publisher<std_msgs::UInt32>> publisher;
    {
        hypha::Node node("session_left");
        publisher.emplace(node.advertise<std_msgs::UInt32>(hypha::Topic{"/session_left"}));
        std_msgs::UInt32 value{};
        ASSERT_TRUE(publisher->publish(value));
    }
    ASSERT_EQ(hypha::backend::nodeHandle(), nullptr);
    std_msgs::UInt32 value{};
    EXPECT_FALSE(publisher->publish(value)) << "nothing is sent on a left session";
    EXPECT_EQ(publisher->lastPublishStatus(), hypha::Status::InvalidState);
}

}  // namespace
