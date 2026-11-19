#include "test_types.h"

#include <gtest/gtest.h>

#include <optional>

using hypha_test::Reading;
using hypha_test::testTopic;

namespace {

class PipelineFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("pipeline_test");
    }

    std::optional<hypha::Node> node;
};

}  // namespace

TEST_F(PipelineFixture, GuardrailStartupValidation)
{
    // Topic name exceeding 128 bytes
    std::string longTopic(130, 'a');
    const auto  advertise = [](const std::string& topic) {
        return hypha_test::refusalOf([&topic] {
            static_cast<void>(hypha::Node().advertise<Reading>(hypha::Topic{topic}));
        });
    };
    EXPECT_NE(advertise(longTopic).find("InvalidTopic"), std::string::npos);

    // Topic name with invalid characters
    EXPECT_NE(advertise("invalid topic name!").find("InvalidTopic"), std::string::npos);

    // A relative name is not a canonical public topic. The refusal names the topic.
    const std::string relative = advertise("node/example");
    EXPECT_NE(relative.find("InvalidTopic"), std::string::npos) << relative;
    EXPECT_NE(relative.find("topic 'node/example'"), std::string::npos) << relative;

    const std::string relativeSub = hypha_test::refusalOf([] {
        static_cast<void>(hypha::Node().subscribe<Reading>(hypha::Topic{"node/example"}));
    });
    EXPECT_NE(relativeSub.find("InvalidTopic"), std::string::npos) << relativeSub;
}
