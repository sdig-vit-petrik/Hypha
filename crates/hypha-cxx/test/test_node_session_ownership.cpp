// The session is shared by every Node that joined it and left by the last of them (#97).

#include "hypha/hypha.h"

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_msgs/UInt32.h>

#include <filesystem>
#include <fstream>
#include <memory>
#include <optional>
#include <string>

namespace {

/// A parameter file `session_file_<tag>.toml` in the temporary directory with `x = <value>`.
std::string parameterFile(const std::string& tag, int value)
{
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / ("session_file_" + tag + ".toml");
    std::ofstream(path) << "x = " << value << "\n";
    return path.string();
}

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

// One parameter file per process, read once (cpp-api.md section 9, #208): a Node that names
// no file shares the process configuration, the same file joins, another file is refused.

TEST(NodeParameterFile, aSecondNodeWithoutAFileSharesTheFirstNodesFile)
{
    const std::string file = parameterFile("share", 1);
    hypha::Node       first("file_share_alpha", file);
    hypha::Node       second("file_share_beta");
    EXPECT_EQ(hypha::nodeName(), "file_share_alpha");
    EXPECT_EQ(hypha::config().required<int>("x"), 1);
    std::filesystem::remove(file);
}

TEST(NodeParameterFile, theSameFileTwiceJoins)
{
    const std::string file = parameterFile("same", 2);
    hypha::Node       first("file_same_alpha", file);
    hypha::Node       second("file_same_beta", file);
    EXPECT_EQ(hypha::config().required<int>("x"), 2);
    std::filesystem::remove(file);
}

TEST(NodeParameterFile, aDifferentFileIsRefusedAndTheMessageNamesBothFiles)
{
    const std::string held = parameterFile("held", 3);
    const std::string other = parameterFile("other", 4);
    {
        hypha::Node       first("file_other_alpha", held);
        const std::string refusal = hypha_test::refusalOf([&other] {
            hypha::Node second("file_other_beta", other);
        });
        EXPECT_NE(refusal.find(held), std::string::npos) << refusal;
        EXPECT_NE(refusal.find(other), std::string::npos) << refusal;
        EXPECT_EQ(hypha::config().required<int>("x"), 3) << "the first file stays";
    }
    EXPECT_EQ(hypha::backend::nodeHandle(), nullptr) << "a refused Node did not join";
    std::filesystem::remove(held);
    std::filesystem::remove(other);
}

TEST(NodeParameterFile, aFileAfterANodeWithoutOneIsRefusedAndTheMessageNamesIt)
{
    const std::string file = parameterFile("late", 5);
    hypha::Node       first("file_late_alpha");
    const std::string refusal = hypha_test::refusalOf([&file] {
        hypha::Node second("file_late_beta", file);
    });
    EXPECT_NE(refusal.find(file), std::string::npos) << refusal;
    std::filesystem::remove(file);
}

TEST(NodeParameterFile, aConfigContextIsSharedByTwoNodesWithoutAFile)
{
    const std::string    file = parameterFile("context", 6);
    hypha::ConfigContext context(file);
    {
        hypha::Node first("file_context_alpha");
        hypha::Node second("file_context_beta");
        EXPECT_EQ(hypha::nodeName(), "file_context_alpha");
        EXPECT_EQ(hypha::config().required<int>("x"), 6);
    }
    EXPECT_EQ(hypha::config().required<int>("x"), 6) << "the context outlives its Nodes";
    {
        hypha::Node again("file_context_gamma", file);
        EXPECT_EQ(hypha::config().required<int>("x"), 6);
    }
    std::filesystem::remove(file);
}

TEST(NodeParameterFile, aConfigContextRefusesANodeWithAnotherFileAndNamesBoth)
{
    const std::string held = parameterFile("context_held", 7);
    const std::string other = parameterFile("context_other", 8);
    {
        hypha::ConfigContext context(held);
        const std::string    refusal = hypha_test::refusalOf([&other] {
            hypha::Node node("file_context_other", other);
        });
        EXPECT_NE(refusal.find(held), std::string::npos) << refusal;
        EXPECT_NE(refusal.find(other), std::string::npos) << refusal;
        const std::string contextRefusal = hypha_test::refusalOf([&other] {
            hypha::ConfigContext second(other);
        });
        EXPECT_NE(contextRefusal.find(held), std::string::npos) << contextRefusal;
        EXPECT_NE(contextRefusal.find(other), std::string::npos) << contextRefusal;
    }
    std::filesystem::remove(held);
    std::filesystem::remove(other);
}

}  // namespace
