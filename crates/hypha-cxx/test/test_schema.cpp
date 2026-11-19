#include "test_types.h"

#include <gtest/gtest.h>

#include <chrono>
#include <optional>
#include <string>
#include <thread>

TEST(Schema, LookupUsesTheExactTopicTypeAndMd5Identity)
{
    hypha::Node node("schema_test");

    const std::string topic = hypha_test::testTopic("/raw/schema");
    EXPECT_FALSE(
        hypha::lookupDefinition(topic, "std_msgs/Bool", "00000000000000000000000000000000")
            .has_value()
    );

    hypha::RawPublisher publisher = hypha::rawPublisher(
                                        topic,
                                        {.dataType = "std_msgs/Bool",
                                         .md5 = "8b94c1b53db61fb6aed406028ad6332a",
                                         .definition = "bool data\n"}
    )
                                        .value();
    ASSERT_TRUE(publisher.isValid());

    std::optional<std::string>                  definition;
    const std::chrono::steady_clock::time_point deadline =
        std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (!definition.has_value() && std::chrono::steady_clock::now() < deadline) {
        definition =
            hypha::lookupDefinition(topic, "std_msgs/Bool", "8b94c1b53db61fb6aed406028ad6332a");
        if (!definition.has_value()) {
            std::this_thread::sleep_for(std::chrono::milliseconds(25));
        }
    }

    ASSERT_TRUE(definition.has_value());
    EXPECT_EQ(*definition, "bool data\n");
    EXPECT_FALSE(
        hypha::lookupDefinition(
            topic + "/other", "std_msgs/Bool", "8b94c1b53db61fb6aed406028ad6332a"
        )
            .has_value()
    );
}
