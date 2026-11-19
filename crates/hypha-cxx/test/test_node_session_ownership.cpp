// A Node releases the bus session only when its own constructor created it.

#include "hypha/hypha.h"

#include <gtest/gtest.h>

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

}  // namespace
