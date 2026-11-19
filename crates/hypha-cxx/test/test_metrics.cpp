// The named metrics that hypha::metrics() and hypha::metric() read.
// The names are part of the C ABI. A name the node does not emit is absent rather than zero.

#include "test_types.h"

#include <gtest/gtest.h>

#include <optional>
#include <string>
#include <vector>

using namespace hypha::literals;

namespace {

std::vector<std::string> namesOf(const std::vector<hypha::Metric>& list)
{
    std::vector<std::string> names;
    names.reserve(list.size());
    for (const hypha::Metric& entry : list) {
        names.push_back(entry.name);
    }
    return names;
}

TEST(Metrics, ListLogNamesBeforeANodeExists)
{
    ASSERT_EQ(hypha::backend::nodeHandle(), nullptr) << "an earlier test left a Node behind";

    const std::expected<std::vector<hypha::Metric>, hypha::Status> list = hypha::metrics();

    ASSERT_TRUE(list.has_value());
    EXPECT_EQ(
        namesOf(*list),
        (std::vector<std::string>{
            "log.lines_written",
            "log.write_failures",
            "log.rejected_lines",
        })
    );
}

TEST(Metrics, ListNodeNamesWithANode)
{
    hypha::Node node("metrics_names");

    const std::expected<std::vector<hypha::Metric>, hypha::Status> list = hypha::metrics();

    ASSERT_TRUE(list.has_value());
    const std::vector<std::string> names = namesOf(*list);
    ASSERT_FALSE(names.empty());
    EXPECT_EQ(names.front(), "node.callbacks_invoked");
    EXPECT_EQ(names.back(), "log.rejected_lines");
    EXPECT_TRUE(hypha::metric("node.callbacks_invoked").has_value());
}

TEST(Metrics, UnknownNameIsAbsent)
{
    EXPECT_EQ(hypha::metric("no.such.metric"), std::nullopt);
}

}  // namespace
