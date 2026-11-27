// Publisher::publishOrWarn() through the log engine: a refusal is one WARN line, naming the
// topic and the status with the count so far, at most once a second for each publisher.
// test_application_skeleton.cpp holds the build with HYPHA_NO_LOGGING.

#include "test_types.h"

#include <gtest/gtest.h>

#include <chrono>
#include <optional>
#include <string>
#include <thread>

using hypha_test::Reading;
using hypha_test::testTopic;

namespace {

int countOccurrences(const std::string& text, const std::string& what)
{
    int count = 0;
    for (size_t at = text.find(what); at != std::string::npos; at = text.find(what, at + 1)) {
        ++count;
    }
    return count;
}

}  // namespace

TEST(PublishOrWarnLog, ARefusalIsOneWarnLineAPeriodWithItsCount)
{
    ASSERT_TRUE(hypha::disableSimTime());
    std::optional<hypha::Node> node;
    node.emplace("publish_or_warn_log_test");
    auto first = node->advertise<Reading>(hypha::Topic{testTopic("/log_first")});
    auto second = node->advertise<Reading>(hypha::Topic{testTopic("/log_second")});
    node.reset();  // a publisher of a session that has left refuses with InvalidState

    ::testing::internal::CaptureStderr();
    for (int call = 0; call < 5; ++call) {
        EXPECT_FALSE(first.publishOrWarn(Reading{call, "a"}));
    }
    EXPECT_FALSE(second.publishOrWarn(Reading{1, "b"}));
    std::this_thread::sleep_for(std::chrono::milliseconds(1100));
    EXPECT_FALSE(first.publishOrWarn(Reading{6, "a"}));
    const std::string output = ::testing::internal::GetCapturedStderr();

    EXPECT_EQ(countOccurrences(output, "message not sent: InvalidState"), 3) << output;
    EXPECT_EQ(countOccurrences(output, "[WARN]"), countOccurrences(output, "message not sent"))
        << output;
    EXPECT_EQ(countOccurrences(output, "'" + testTopic("/log_first") + "'"), 2) << output;
    EXPECT_EQ(countOccurrences(output, "'" + testTopic("/log_second") + "'"), 1) << output;
    EXPECT_EQ(countOccurrences(output, "(1 refused so far)"), 2) << output;
    EXPECT_EQ(countOccurrences(output, "(6 refused so far)"), 1) << output;
}
