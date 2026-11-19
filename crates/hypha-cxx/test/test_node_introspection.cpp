// The node introspection snapshot holds the configuration rows and the
// subscriber rows of a node. These tests read the rows back by their first
// word from the plain dump of the snapshot.

#include "hypha/introspection.h"

#include "introspection_text.h"
#include "test_types.h"

#include <gtest/gtest.h>

#include <filesystem>
#include <fstream>
#include <iterator>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

using Words = std::vector<std::string>;

Words wordsOf(const std::string& line)
{
    std::istringstream stream(line);
    return Words(std::istream_iterator<std::string>(stream), std::istream_iterator<std::string>());
}

/// The words of the first line whose first word is @p key. Empty when none.
Words rowOf(const std::string& text, const std::string& key)
{
    std::istringstream lines(text);
    std::string        line;
    while (std::getline(lines, line)) {
        Words      words = wordsOf(line);
        const bool isMatch = !words.empty() && words.front() == key;
        if (isMatch) {
            return words;
        }
    }
    return {};
}

class NodeIntrospectionFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_node_introspection_test.toml";
        std::ofstream output(this->path);
        output << "count=7\n";
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("node_introspection_test", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

}  // namespace

TEST_F(NodeIntrospectionFixture, ConfigReadWithFallbackShowsFallbackAsDefault)
{
    EXPECT_EQ(hypha::config().optional<int>("count", 0), 7);
    EXPECT_EQ(hypha::config().optional<int>("missing", 42), 42);

    const std::string text = test_support::introspectionText();
    EXPECT_EQ(rowOf(text, "count"), (Words{"count", "7", "0", "parameter"}));
    EXPECT_EQ(rowOf(text, "missing"), (Words{"missing", "42", "42", "default"}));
}

TEST_F(NodeIntrospectionFixture, RequiredConfigShowsNoDefault)
{
    EXPECT_EQ(hypha::config().required<int>("count"), 7);

    EXPECT_EQ(
        rowOf(test_support::introspectionText(), "count"), (Words{"count", "7", "-", "parameter"})
    );
}

TEST_F(NodeIntrospectionFixture, TypedSubscriberRowShowsMessageType)
{
    const std::string topic = hypha_test::testTopic("/readings");
    auto              sub = hypha::Node().subscribe<hypha_test::Reading>(hypha::Topic{topic});

    const Words row = rowOf(test_support::introspectionText(), topic);
    ASSERT_GE(row.size(), 2U);
    EXPECT_EQ(row[1], "hypha_test/Reading");
}

TEST_F(NodeIntrospectionFixture, TypedWildcardSubscriberRowShowsMessageType)
{
    const std::string topic = hypha_test::testTopic("/wild_introspection");
    auto sub = hypha::Node().subscribe<hypha_test::Reading>(hypha::Topic{topic + "/*"});

    const Words row = rowOf(test_support::introspectionText(), topic + "/*");
    ASSERT_GE(row.size(), 2U);
    EXPECT_EQ(row[1], "hypha_test/Reading");
}

TEST_F(NodeIntrospectionFixture, TheSnapshotNamesThisNode)
{
    const auto snapshot = hypha::introspect();
    ASSERT_TRUE(snapshot.has_value()) << snapshot.error().asInt();
    EXPECT_EQ(snapshot->node, "node_introspection_test");
    EXPECT_FALSE(snapshot->hypha.empty());
}

TEST(NodeIntrospectionWithoutNode, IntrospectBeforeInitThrows)
{
    EXPECT_THROW((void)hypha::introspect(), std::runtime_error);
}
