// What the ApplicationNode skeleton of docs/ros1-migration-guide.md section 10.2 no longer
// forces on a node (#213): a transform buffer as a plain member, a checked send in one line,
// a bound argument on hypha::bind, a RosTime from seconds or nanoseconds, and a call that
// says the clock has a value. Each of these first failed to compile or to behave.

#include "hypha/hypha.h"
#include "hypha/transform_buffer.h"

#include "test_types.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <cstdlib>
#include <functional>
#include <limits>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

using hypha_test::Reading;
using hypha_test::testTopic;

namespace {

/// One object that several inputs call into: which input a sample came from is the
/// bound argument.
class Inputs
{
  public:
    void onSample(size_t input, int value)
    {
        this->seen.emplace_back(input, value);
    }

    void onPointer(const std::string* name, int value)
    {
        this->names.push_back(*name + ":" + std::to_string(value));
    }

    int sum(int base, int first, int second) const
    {
        return base + first + second;
    }

    std::vector<std::pair<size_t, int>> seen;
    std::vector<std::string>            names;
};

int countOccurrences(const std::string& text, const std::string& what)
{
    int count = 0;
    for (size_t at = text.find(what); at != std::string::npos; at = text.find(what, at + 1)) {
        ++count;
    }
    return count;
}

}  // namespace

TEST(Bind, ABoundArgumentComesBeforeTheCallersArguments)
{
    Inputs inputs;
    auto   second = hypha::bind<&Inputs::onSample>(inputs, size_t{1});
    auto   third = hypha::bind<&Inputs::onSample>(inputs, size_t{2});
    second(10);
    third(20);
    second(11);
    ASSERT_EQ(inputs.seen.size(), 3U);
    EXPECT_EQ(inputs.seen[0], (std::pair<size_t, int>{1, 10}));
    EXPECT_EQ(inputs.seen[1], (std::pair<size_t, int>{2, 20}));
    EXPECT_EQ(inputs.seen[2], (std::pair<size_t, int>{1, 11}));
}

TEST(Bind, ABoundPointerAndSeveralBoundArgumentsWork)
{
    Inputs            inputs;
    const std::string name = "front";
    hypha::bind<&Inputs::onPointer>(inputs, &name)(7);
    ASSERT_EQ(inputs.names.size(), 1U);
    EXPECT_EQ(inputs.names[0], "front:7");
    EXPECT_EQ(hypha::bind<&Inputs::sum>(inputs, 100, 20)(3), 123);
}

TEST(Bind, TheBoundCallbackIsCopyableAndStoredByAStdFunction)
{
    Inputs                   inputs;
    std::function<void(int)> stored = hypha::bind<&Inputs::onSample>(inputs, size_t{5});
    const auto               copy = stored;
    copy(1);
    stored(2);
    ASSERT_EQ(inputs.seen.size(), 2U);
    EXPECT_EQ(inputs.seen[1].first, 5U);
}

TEST(Bind, ASubscriberCallsTheMemberWithItsInputIndex)
{
    ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1'000'000'000)));
    hypha::Node node("bind_arguments_test");

    struct Collector
    {
        std::vector<size_t> inputs;

        void onReading(size_t input, const hypha::MessagePtr<Reading>&)
        {
            this->inputs.push_back(input);
        }
    } collector;

    auto first = node.subscribe<Reading>(
        hypha::Topic{testTopic("/first")}, hypha::bind<&Collector::onReading>(collector, size_t{0})
    );
    auto second = node.subscribe<Reading>(
        hypha::Topic{testTopic("/second")}, hypha::bind<&Collector::onReading>(collector, size_t{1})
    );
    auto firstOut = node.advertise<Reading>(hypha::Topic{testTopic("/first")});
    auto secondOut = node.advertise<Reading>(hypha::Topic{testTopic("/second")});
    ASSERT_TRUE(secondOut.publish(Reading{1, "b"}));
    ASSERT_TRUE(firstOut.publish(Reading{2, "a"}));
    ASSERT_TRUE(hypha_test::runUntil(node, [&collector] {
        return collector.inputs.size() == 2;
    }));
    EXPECT_EQ(collector.inputs.size(), 2U);
    EXPECT_TRUE(
        (collector.inputs == std::vector<size_t>{0, 1})
        || (collector.inputs == std::vector<size_t>{1, 0})
    );
}

TEST(RosTimeFrom, SecondsAndNanosecondsGiveTheWireStamp)
{
    const hypha_msgs::RosTime fromSeconds = hypha::rosTimeFromSeconds(12.5);
    EXPECT_EQ(fromSeconds.secs, 12U);
    EXPECT_EQ(fromSeconds.nsecs, 500'000'000U);
    const hypha_msgs::RosTime fromNanos = hypha::rosTimeFromNanos(3'000'000'007);
    EXPECT_EQ(fromNanos.secs, 3U);
    EXPECT_EQ(fromNanos.nsecs, 7U);
}

TEST(RosTimeFrom, AValueTheWireCannotCarryThrowsAsToRosTimeDoes)
{
    EXPECT_THROW(hypha::rosTimeFromSeconds(-1.0), std::out_of_range);
    EXPECT_THROW(hypha::rosTimeFromNanos(-1), std::out_of_range);
    EXPECT_THROW(hypha::rosTimeFromSeconds(5.0e9), std::out_of_range);
    EXPECT_THROW(
        hypha::rosTimeFromSeconds(std::numeric_limits<double>::quiet_NaN()), std::invalid_argument
    );
}

namespace {

/// Members as the guide's skeleton declares them: no optional.
struct SkeletonMembers
{
    hypha::TransformBuffer            tf;
    hypha::StaticTransformBroadcaster statics;
    hypha::TransformBroadcaster       broadcaster;
};

}  // namespace

TEST(DefaultTransforms, AMemberIsCreatedInSetupByAssignment)
{
    ASSERT_TRUE(hypha::disableSimTime());
    hypha::Node     node("default_transforms_test");
    SkeletonMembers members;
    EXPECT_FALSE(members.tf.isValid());
    members.tf = hypha::TransformBuffer(
        node, hypha::TransformTopics{.tf = testTopic("/tf"), .tfStatic = testTopic("/tf_static")}
    );
    EXPECT_TRUE(members.tf.isValid());
    const auto found = members.tf.lookupLatest("map", "base");
    ASSERT_FALSE(found.has_value());
    EXPECT_EQ(found.error(), hypha::TransformError::UnknownFrame);
}

TEST(DefaultTransforms, ALookupOnABufferThatWasNeverCreatedThrowsAndSaysSo)
{
    const hypha::TransformBuffer never;
    EXPECT_FALSE(never.isValid());
    try {
        static_cast<void>(never.lookupLatest("map", "base"));
        FAIL() << "a lookup on a default-constructed buffer must throw";
    } catch (const std::runtime_error& error) {
        EXPECT_NE(std::string(error.what()).find("never created"), std::string::npos)
            << error.what();
    }
}

TEST(DefaultTransforms, ABroadcasterThatWasNeverCreatedSendsNothingAndSaysWhy)
{
    hypha::StaticTransformBroadcaster statics;
    hypha::TransformBroadcaster       broadcaster;
    geometry_msgs::TransformStamped   transform{};
    transform.header.frame_id = "map";
    transform.child_frame_id = "base";
    transform.transform.rotation.w = 1.0;
    EXPECT_FALSE(statics.send(transform));
    EXPECT_EQ(statics.lastPublishStatus(), hypha::Status::InvalidState);
    EXPECT_FALSE(broadcaster.send(transform));
    EXPECT_EQ(broadcaster.lastPublishStatus(), hypha::Status::InvalidState);
}

class PublishOrWarn : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::disableSimTime());
    }
};

TEST_F(PublishOrWarn, ASentMessageReturnsTrueAndWritesNoLine)
{
    hypha::Node node("publish_or_warn_test");
    auto        out = node.advertise<Reading>(hypha::Topic{testTopic("/sent")});
    ::testing::internal::CaptureStderr();
    EXPECT_TRUE(out.publishOrWarn(Reading{1, "a"}));
    EXPECT_EQ(countOccurrences(::testing::internal::GetCapturedStderr(), "not sent"), 0);
}

// This binary is built with HYPHA_NO_LOGGING: the line goes to standard error, once for each
// publisher. The log engine's throttle and count: test_publish_or_warn_log.cpp.
TEST_F(PublishOrWarn, ARefusedMessageReturnsFalseAndSaysSoOnceForEachPublisher)
{
    std::optional<hypha::Node> node;
    node.emplace("publish_or_warn_test");
    auto first = node->advertise<Reading>(hypha::Topic{testTopic("/refused_first")});
    auto second = node->advertise<Reading>(hypha::Topic{testTopic("/refused_second")});
    node.reset();  // a publisher of a session that has left refuses with InvalidState
    ::testing::internal::CaptureStderr();
    for (int call = 0; call < 5; ++call) {
        EXPECT_FALSE(first.publishOrWarn(Reading{call, "a"}));
    }
    EXPECT_FALSE(second.publishOrWarn(Reading{1, "b"}));
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(first.lastPublishStatus(), hypha::Status::InvalidState);
    EXPECT_EQ(countOccurrences(output, "message not sent"), 2) << output;
    EXPECT_EQ(countOccurrences(output, "'" + testTopic("/refused_first") + "'"), 1) << output;
    EXPECT_EQ(countOccurrences(output, "'" + testTopic("/refused_second") + "'"), 1) << output;
    EXPECT_NE(output.find("InvalidState"), std::string::npos) << output;
}

TEST_F(PublishOrWarn, ADisabledPublisherSendsNothingAndSaysNothing)
{
    hypha::Publisher<Reading> disabled;
    ::testing::internal::CaptureStderr();
    EXPECT_FALSE(disabled.publishOrWarn(Reading{1, "a"}));
    EXPECT_EQ(countOccurrences(::testing::internal::GetCapturedStderr(), "not sent"), 0);
}

TEST(ClockValid, IsTrueOnHostTime)
{
    ASSERT_TRUE(hypha::disableSimTime());
    EXPECT_TRUE(hypha::isClockValid());
}

TEST(ClockValid, IsFalseOnSimulatedTimeUntilItHasAValue)
{
    ASSERT_TRUE(hypha::disableSimTime());
    ASSERT_EQ(::setenv("HYPHA_USE_SIM_TIME", "1", 1), 0);
    {
        hypha::Node node("clock_valid_test");
        EXPECT_TRUE(hypha::isSimTime());
        EXPECT_FALSE(hypha::isClockValid()) << "the node's clock is the placeholder zero";
        ASSERT_TRUE(hypha::setSimTime(hypha::Time::fromNanos(2'000'000'000)));
        EXPECT_TRUE(hypha::isClockValid());
    }
    ASSERT_EQ(::unsetenv("HYPHA_USE_SIM_TIME"), 0);
    ASSERT_TRUE(hypha::disableSimTime());
}

TEST(ClockValid, IsTrueWhenSimulatedTimeIsEnabledAtAValue)
{
    ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1'000'000'000)));
    EXPECT_TRUE(hypha::isClockValid());
    ASSERT_TRUE(hypha::disableSimTime());
}
