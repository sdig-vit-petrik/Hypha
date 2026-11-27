// Topic name overloading: one topic name carries more than one schema, and each schema is
// its own stream (#209), so a typed reader never meets a frame of another type. The case
// that matters is a migration: two versions of one message type (one type name, two MD5s)
// on one topic. These tests observe what a user sees: delivery, the verdicts of cycle
// inputs, the outputs, a wildcard, the ~/state rows, and what the library says to a reader
// whose version nobody publishes.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include <hypha_msgs/NodeState.h>

#include "test_types.h"

#include <gtest/gtest.h>

#include <chrono>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <functional>
#include <optional>
#include <string>
#include <thread>
#include <vector>

namespace hypha_test {

/// Version 2 of Reading: the same type name, one field more, so another MD5.
struct ReadingV2
{
    int         value{0};
    std::string label;
    double      weight{0.0};
};

}  // namespace hypha_test

namespace ros::message_traits {

template <>
struct DataType<hypha_test::ReadingV2>
{
    static const char* value()
    {
        return "hypha_test/Reading";
    }

    static const char* value(const hypha_test::ReadingV2&)
    {
        return value();
    }
};

template <>
struct MD5Sum<hypha_test::ReadingV2>
{
    static const char* value()
    {
        return "f79835def461350458ead76f07016dd7";
    }

    static const char* value(const hypha_test::ReadingV2&)
    {
        return value();
    }
};

template <>
struct Definition<hypha_test::ReadingV2>
{
    static const char* value()
    {
        return "int32 value\nstring label\nfloat64 weight\n";
    }

    static const char* value(const hypha_test::ReadingV2&)
    {
        return value();
    }
};

}  // namespace ros::message_traits

namespace ros::serialization {

template <>
struct Serializer<hypha_test::ReadingV2>
{
    template <typename Stream, typename T>
    inline static void allInOne(Stream& stream, T m)
    {
        stream.next(m.value);
        stream.next(m.label);
        stream.next(m.weight);
    }

    ROS_DECLARE_ALLINONE_SERIALIZER
};

}  // namespace ros::serialization

using hypha_test::Command;
using hypha_test::Reading;
using hypha_test::ReadingV2;
using hypha_test::testTopic;
using namespace hypha::literals;

namespace {

/// Keeps the value of each Reading it receives.
struct KeepReadings
{
    std::vector<int>* values;

    void operator()(const hypha::MessagePtr<Reading>& message) const
    {
        this->values->push_back(message->value);
    }
};

/// Keeps the target of each Command it receives.
struct KeepCommands
{
    std::vector<double>* targets;

    void operator()(const hypha::MessagePtr<Command>& message) const
    {
        this->targets->push_back(message->target);
    }
};

Reading reading(int value)
{
    Reading result;
    result.value = value;
    return result;
}

Command command(double target)
{
    Command result;
    result.target = target;
    return result;
}

/// True when a subscriber refused no frame.
bool refusedNothing(const hypha::SubscriberMetrics& metrics)
{
    return metrics.sequenceGap == 0 && metrics.invalidFrame == 0 && metrics.corruptPayload == 0
        && metrics.schemaMismatch == 0 && metrics.invalidTimestamp == 0;
}

/// Runs short slices of simulated time until @p done holds, or fails after a generous
/// real-time bound. Ages are judged on the simulated clock; the real sleep only gives
/// the transport time to deliver.
bool runUntil(hypha::Node& node, const std::function<bool()>& done)
{
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(20);
    while (!done()) {
        if (std::chrono::steady_clock::now() >= deadline) {
            return false;
        }
        hypha_test::advance(10_ms);
        if (!node.runFor(1_ms).ok()) {
            return false;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    return true;
}

/// Runs @p slices slices of 10 ms of simulated time, with no event awaited.
bool runSlices(hypha::Node& node, int slices)
{
    for (int slice = 0; slice < slices; ++slice) {
        hypha_test::advance(10_ms);
        if (!node.runFor(1_ms).ok()) {
            return false;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    return true;
}

/// A node on a parameter file whose port tables `joint` (input and output) name the topic
/// of the running test.
class OverloadingFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->topic = testTopic("/joint");
        this->path = std::filesystem::temp_directory_path() / "hypha_topic_overloading_test.toml";
        std::ofstream output(this->path);
        output << "[ports.inputs.joint]\ntopic='" << this->topic << "'\nsilence_ms = 200\n"
               << "[ports.outputs.joint]\ntopic='" << this->topic << "'\n";
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("topic_overloading_test", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
        static_cast<void>(hypha::disableSimTime());
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
    std::string                topic;
};

/// One port name, two input types: the case of #209. The step records what it saw.
class JointCycle final : public hypha_test::StepCycle
{
  public:
    JointCycle()
        : hypha_test::StepCycle("joint_cycle"), latest(*this, "joint"), arrivals(*this, "joint")
    {
    }

    hypha::Latest<Reading>   latest;
    hypha::Arrivals<Command> arrivals;

    int                 steps{0};
    hypha::InputVerdict latestVerdict{hypha::InputVerdict::NeverArrived};
    hypha::InputVerdict arrivalsVerdict{hypha::InputVerdict::NeverArrived};
    std::optional<int>  lastReading;
    std::vector<double> commands;

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        this->latestVerdict = this->latest.verdict();
        this->arrivalsVerdict = this->arrivals.verdict();
        if (this->latest.usable()) {
            this->lastReading = (*this->latest).value;
        }
        for (const Command& sample : this->arrivals.samples()) {
            this->commands.push_back(sample.target);
        }
    }
};

/// One port name, two output types.
class JointOutputs final : public hypha_test::StepCycle
{
  public:
    JointOutputs()
        : hypha_test::StepCycle("joint_outputs"), readings(*this, "joint"), commands(*this, "joint")
    {
    }

    hypha::Output<Reading> readings;
    hypha::Output<Command> commands;
    int                    steps{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        this->readings.send(reading(this->steps));
        this->commands.send(command(0.5 * this->steps));
    }
};

}  // namespace

TEST_F(OverloadingFixture, TwoTypesOnOneNameAreBothCreatedAndEachSubscriberReadsItsOwnType)
{
    std::vector<int>    readings;
    std::vector<double> commands;

    auto readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    ASSERT_TRUE(readingPublisher.enabled());
    ASSERT_TRUE(commandPublisher.enabled());
    auto readingSubscriber =
        hypha::Node().subscribe<Reading>(hypha::Topic{this->topic}, KeepReadings{&readings});
    auto commandSubscriber =
        hypha::Node().subscribe<Command>(hypha::Topic{this->topic}, KeepCommands{&commands});
    ASSERT_TRUE(readingSubscriber.enabled());
    ASSERT_TRUE(commandSubscriber.enabled());
    EXPECT_EQ(readingSubscriber.topic(), commandSubscriber.topic());

    constexpr int FRAMES = 6;
    for (int index = 1; index <= FRAMES; ++index) {
        ASSERT_TRUE(readingPublisher.publish(reading(index)));
        ASSERT_TRUE(commandPublisher.publish(command(0.5 * index)));
    }
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return readings.size() >= FRAMES && commands.size() >= FRAMES;
    }));
    ASSERT_TRUE(runSlices(*this->node, 5));

    EXPECT_EQ(readings, (std::vector<int>{1, 2, 3, 4, 5, 6}));
    EXPECT_EQ(commands, (std::vector<double>{0.5, 1.0, 1.5, 2.0, 2.5, 3.0}));
    EXPECT_EQ(readingSubscriber.messageCount(), static_cast<uint64_t>(FRAMES));
    EXPECT_EQ(commandSubscriber.messageCount(), static_cast<uint64_t>(FRAMES));
    ASSERT_TRUE(readingSubscriber.latestSample().has_value());
    EXPECT_EQ(readingSubscriber.latestSample()->message->value, FRAMES);
    ASSERT_TRUE(commandSubscriber.latestSample().has_value());
    EXPECT_DOUBLE_EQ(commandSubscriber.latestSample()->message->target, 3.0);
}

TEST_F(OverloadingFixture, SubscribersCreatedBeforeThePublishersReadTheirOwnTypeToo)
{
    std::vector<int>    readings;
    std::vector<double> commands;

    auto commandSubscriber =
        hypha::Node().subscribe<Command>(hypha::Topic{this->topic}, KeepCommands{&commands});
    auto readingSubscriber =
        hypha::Node().subscribe<Reading>(hypha::Topic{this->topic}, KeepReadings{&readings});
    ASSERT_TRUE(readingSubscriber.enabled());
    ASSERT_TRUE(commandSubscriber.enabled());
    auto commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    auto readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});

    ASSERT_TRUE(commandPublisher.publish(command(2.5)));
    ASSERT_TRUE(readingPublisher.publish(reading(9)));
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return !readings.empty() && !commands.empty();
    }));
    EXPECT_EQ(readings, std::vector<int>{9});
    EXPECT_EQ(commands, std::vector<double>{2.5});
}

TEST_F(OverloadingFixture, ASubscriberOfAnotherTypeThanTheOnlyPublisherMeetsNoFrameOfIt)
{
    std::vector<double> commands;
    auto readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    ASSERT_TRUE(readingPublisher.enabled());

    ::testing::internal::CaptureStderr();
    auto commandSubscriber =
        hypha::Node().subscribe<Command>(hypha::Topic{this->topic}, KeepCommands{&commands});
    ASSERT_TRUE(commandSubscriber.enabled());
    for (int index = 1; index <= 5; ++index) {
        ASSERT_TRUE(readingPublisher.publish(reading(index)));
    }
    ASSERT_TRUE(runSlices(*this->node, 20));
    const std::string afterwards = ::testing::internal::GetCapturedStderr();

    EXPECT_TRUE(commands.empty());
    EXPECT_EQ(commandSubscriber.messageCount(), 0U);
    EXPECT_FALSE(commandSubscriber.latestSample().has_value());
    EXPECT_EQ(afterwards.find("refused a frame"), std::string::npos) << afterwards;
    EXPECT_TRUE(refusedNothing(commandSubscriber.metrics())) << "nothing refused, nothing counted";
}

TEST_F(OverloadingFixture, ALatestAndAnArrivalsOfTwoTypesOnOnePortNameAreEachUsable)
{
    auto       readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto       commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    JointCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node).has_value());

    ASSERT_TRUE(readingPublisher.publish(reading(11)));
    ASSERT_TRUE(commandPublisher.publish(command(1.5)));
    ASSERT_TRUE(commandPublisher.publish(command(2.5)));
    ASSERT_TRUE(runUntil(*this->node, [&cycle] {
        return cycle.lastReading.has_value() && cycle.commands.size() >= 2U;
    }));

    EXPECT_EQ(cycle.lastReading, 11);
    EXPECT_EQ(cycle.commands, (std::vector<double>{1.5, 2.5}));
    EXPECT_EQ(cycle.latest.received(), 1U) << "the Latest counts its own type only";
    EXPECT_EQ(cycle.arrivals.received(), 2U) << "the Arrivals counts its own type only";
    EXPECT_EQ(cycle.latestVerdict, hypha::InputVerdict::Usable);
    EXPECT_EQ(cycle.arrivalsVerdict, hypha::InputVerdict::Usable);
}

TEST_F(OverloadingFixture, EachInputOfTheNameHasItsOwnVerdictAndItsOwnSilence)
{
    auto       readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto       commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    JointCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node).has_value());

    // Only the Command type is published: the Latest never arrived, the Arrivals is usable.
    int sent = 0;
    for (int slice = 0; slice < 40; ++slice) {
        ASSERT_TRUE(commandPublisher.publish(command(static_cast<double>(++sent))));
        ASSERT_TRUE(runSlices(*this->node, 1));
    }
    EXPECT_EQ(cycle.latestVerdict, hypha::InputVerdict::NeverArrived);
    EXPECT_FALSE(cycle.latest.has());
    EXPECT_EQ(cycle.latest.received(), 0U);
    EXPECT_EQ(cycle.arrivalsVerdict, hypha::InputVerdict::Usable);
    EXPECT_FALSE(cycle.inputsOk())
        << "the input that never arrived makes the cycle's inputs not ok";

    // One Reading arrives: the Latest is usable, with the value of the Reading.
    ASSERT_TRUE(readingPublisher.publish(reading(5)));
    ASSERT_TRUE(runUntil(*this->node, [&cycle] {
        return cycle.latestVerdict == hypha::InputVerdict::Usable;
    }));
    EXPECT_EQ(cycle.lastReading, 5);

    // The Readings stop while the Commands go on: the Latest goes stale on its own, the
    // Arrivals stays usable.
    ASSERT_TRUE(runUntil(*this->node, [&] {
        static_cast<void>(commandPublisher.publish(command(static_cast<double>(++sent))));
        return cycle.latestVerdict == hypha::InputVerdict::Stale;
    }));
    EXPECT_TRUE(cycle.latest.stale());
    EXPECT_FALSE(cycle.arrivals.stale()) << "a silent type does not make the other one stale";
    EXPECT_EQ(cycle.arrivalsVerdict, hypha::InputVerdict::Usable);

    // The Commands stop while the Readings go on: the verdicts swap.
    int readingValue = 6;
    ASSERT_TRUE(runUntil(*this->node, [&] {
        static_cast<void>(readingPublisher.publish(reading(readingValue++)));
        return cycle.arrivalsVerdict == hypha::InputVerdict::Stale;
    }));
    EXPECT_EQ(cycle.latestVerdict, hypha::InputVerdict::Usable);
    EXPECT_FALSE(cycle.latest.stale());
    EXPECT_TRUE(cycle.arrivals.stale());
    EXPECT_GE(cycle.lastReading.value_or(0), 6);
}

TEST_F(OverloadingFixture, EachTypeOfTheTopicIsARowOfItsOwnInTheState)
{
    auto       commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    JointCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node).has_value());

    for (int slice = 0; slice < 40; ++slice) {
        ASSERT_TRUE(commandPublisher.publish(command(1.0)));
        ASSERT_TRUE(runSlices(*this->node, 1));
    }
    ASSERT_EQ(cycle.arrivalsVerdict, hypha::InputVerdict::Usable);
    ASSERT_EQ(cycle.latestVerdict, hypha::InputVerdict::NeverArrived);

    // `~/state` has one row for each stream: the Arrivals is usable and the Latest never
    // arrived, and the node waits for the Latest.
    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    ASSERT_TRUE(state.has_value());
    EXPECT_EQ(state->state, hypha_msgs::NodeState::WAITING);
    ASSERT_EQ(state->inputs.size(), 2U);
    for (const hypha_msgs::RequiredInput& input : state->inputs) {
        EXPECT_EQ(input.topic, this->topic);
        const bool isReading = input.data_type == "hypha_test/Reading";
        EXPECT_EQ(
            input.md5,
            isReading ? "581e4b1bda6a172aa19e1038a38e01eb" : "c2f7134c29a3ad4daf56996f7fc77409"
        );
        EXPECT_EQ(
            input.verdict,
            isReading ? hypha_msgs::RequiredInput::NEVER_ARRIVED : hypha_msgs::RequiredInput::USABLE
        );
        EXPECT_EQ(input.rejected, 0U);
    }
}

TEST_F(OverloadingFixture, TwoOutputsOfTwoTypesOnOnePortNameReachTheSubscriberOfEachType)
{
    std::vector<int>    readings;
    std::vector<double> commands;
    auto                readingSubscriber =
        hypha::Node().subscribe<Reading>(hypha::Topic{this->topic}, KeepReadings{&readings});
    auto commandSubscriber =
        hypha::Node().subscribe<Command>(hypha::Topic{this->topic}, KeepCommands{&commands});

    JointOutputs cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node).has_value());
    ASSERT_TRUE(readingSubscriber.enabled());
    ASSERT_TRUE(commandSubscriber.enabled());
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return readings.size() >= 3U && commands.size() >= 3U;
    }));

    ASSERT_GE(readings.size(), 3U);
    EXPECT_EQ(readings[0], 1);
    EXPECT_EQ(readings[1], 2);
    EXPECT_EQ(readings[2], 3);
    ASSERT_GE(commands.size(), 3U);
    EXPECT_DOUBLE_EQ(commands[0], 0.5);
    EXPECT_DOUBLE_EQ(commands[1], 1.0);
    EXPECT_DOUBLE_EQ(commands[2], 1.5);
}

TEST_F(OverloadingFixture, ATypedWildcardSubscriberReadsOnlyItsTypeOfAnOverloadedTopic)
{
    std::vector<int>    readings;
    std::vector<double> commands;
    auto readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});

    const std::string pattern = testTopic("/*");
    auto              readingSubscriber =
        hypha::Node().subscribe<Reading>(hypha::Topic{pattern}, KeepReadings{&readings});
    auto commandSubscriber =
        hypha::Node().subscribe<Command>(hypha::Topic{pattern}, KeepCommands{&commands});
    ASSERT_TRUE(readingSubscriber.enabled());
    ASSERT_TRUE(commandSubscriber.enabled());

    ASSERT_TRUE(readingPublisher.publish(reading(3)));
    ASSERT_TRUE(commandPublisher.publish(command(4.5)));
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return !readings.empty() && !commands.empty();
    }));
    ASSERT_TRUE(runSlices(*this->node, 5));
    EXPECT_EQ(readings, std::vector<int>{3});
    EXPECT_EQ(commands, std::vector<double>{4.5});
}

// The queue of a subscriber is one queue for each publisher, not one for the name: a burst of
// one type, which has its own publisher, does not push the frames of another type out of the
// queue of a reader of that other type. The loss of the burst is counted by the readers of
// its own type only.
TEST_F(OverloadingFixture, ABurstOfOneTypeDoesNotPushTheFramesOfTheOtherTypeOutOfItsQueue)
{
    std::vector<int>    readings;
    std::vector<double> commands;
    auto readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    auto readingSubscriber =
        hypha::Node().subscribe<Reading>(hypha::Topic{this->topic}, KeepReadings{&readings});
    auto commandSubscriber =
        hypha::Node().subscribe<Command>(hypha::Topic{this->topic}, KeepCommands{&commands});

    // The first accepted frame of a type starts its count: a loss before it is not counted.
    ASSERT_TRUE(readingPublisher.publish(reading(0)));
    ASSERT_TRUE(commandPublisher.publish(command(-1.0)));
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return readings.size() >= 1U && commands.size() >= 1U;
    }));

    // Five Readings, then more Commands than a queue holds, before the node runs again.
    for (int index = 1; index <= 5; ++index) {
        ASSERT_TRUE(readingPublisher.publish(reading(index)));
    }
    constexpr int BURST = 100;
    static_assert(BURST > 32, "more than the maxDepth of the default policy");
    for (int index = 0; index < BURST; ++index) {
        ASSERT_TRUE(commandPublisher.publish(command(index)));
    }
    ASSERT_TRUE(runSlices(*this->node, 10));

    EXPECT_EQ(readings, (std::vector<int>{0, 1, 2, 3, 4, 5}));
    EXPECT_EQ(readingSubscriber.missingSequences(), 0U);
    EXPECT_EQ(commands.size(), 1U + 32U) << "the queue of the Commands kept the newest 32";
    EXPECT_DOUBLE_EQ(commands.back(), BURST - 1);
    EXPECT_EQ(commandSubscriber.missingSequences(), static_cast<uint64_t>(BURST - 32));
}

// What the library counts for the frames of the other type: nothing. They travel on another
// stream, so they are neither refused nor counted, and `schemaMismatch` keeps meaning a
// sender that stamped another schema than its stream's.
TEST_F(OverloadingFixture, TheFramesOfTheOtherTypeAreNoSchemaMismatchOfAnySubscriber)
{
    std::vector<int>    readings;
    std::vector<double> commands;
    auto readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    auto readingSubscriber =
        hypha::Node().subscribe<Reading>(hypha::Topic{this->topic}, KeepReadings{&readings});
    auto commandSubscriber =
        hypha::Node().subscribe<Command>(hypha::Topic{this->topic}, KeepCommands{&commands});

    // Three of one type, five of the other, drained after each: no loss on either queue.
    for (int index = 1; index <= 3; ++index) {
        ASSERT_TRUE(readingPublisher.publish(reading(index)));
    }
    for (int index = 1; index <= 5; ++index) {
        ASSERT_TRUE(commandPublisher.publish(command(index)));
    }
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return readings.size() >= 3U && commands.size() >= 5U;
    }));
    ASSERT_TRUE(runSlices(*this->node, 5));

    const hypha::SubscriberMetrics ofReadings = readingSubscriber.metrics();
    const hypha::SubscriberMetrics ofCommands = commandSubscriber.metrics();
    EXPECT_EQ(ofReadings.schemaMismatch, 0U);
    EXPECT_EQ(ofCommands.schemaMismatch, 0U);
    EXPECT_EQ(ofReadings.sequenceGap, 0U);
    EXPECT_EQ(ofCommands.sequenceGap, 0U);
    EXPECT_EQ(readingSubscriber.missingSequences(), 0U);
    EXPECT_EQ(commandSubscriber.missingSequences(), 0U);
}

TEST_F(OverloadingFixture, TheFramesOfTheOtherTypeAreNoRejectedFramesOfTheStateRows)
{
    auto       readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto       commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    JointCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node).has_value());

    ASSERT_TRUE(readingPublisher.publish(reading(1)));
    for (int index = 1; index <= 4; ++index) {
        ASSERT_TRUE(commandPublisher.publish(command(index)));
    }
    ASSERT_TRUE(runUntil(*this->node, [&cycle] {
        return cycle.lastReading.has_value() && cycle.commands.size() >= 4U;
    }));
    ASSERT_TRUE(runSlices(*this->node, 5));

    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    ASSERT_TRUE(state.has_value());
    ASSERT_EQ(state->inputs.size(), 2U);
    for (const hypha_msgs::RequiredInput& input : state->inputs) {
        EXPECT_EQ(input.verdict, hypha_msgs::RequiredInput::USABLE) << input.data_type;
        EXPECT_EQ(input.rejected, 0U) << input.data_type;
    }
}

/// A migration on one port name: version 1 of Reading is the cycle's input, and version 2
/// is published beside it.
class MigratingCycle final : public hypha_test::StepCycle
{
  public:
    MigratingCycle() : hypha_test::StepCycle("migrating_cycle"), reading(*this, "joint")
    {
    }

    hypha::Latest<Reading> reading;
    hypha::InputVerdict    verdict{hypha::InputVerdict::NeverArrived};
    std::optional<int>     lastValue;

    void step(const hypha::CycleTime&) override
    {
        this->verdict = this->reading.verdict();
        if (this->reading.usable()) {
            this->lastValue = (*this->reading).value;
        }
    }
};

// Two versions of one type name coexist on one topic: each subscriber reads its own, nothing
// is refused. When version 1's publisher leaves, the input of version 1 goes stale, the node
// keeps running, and after the no-publisher delay (5 s) the log names its version and the one
// that is published. A version 1 publisher that comes back is read again.
TEST_F(OverloadingFixture, TwoVersionsOfOneTypeShareATopicAndAReaderOfTheMissingOneIsTold)
{
    std::vector<int> secondVersion;
    auto v1 = std::optional(hypha::Node().advertise<Reading>(hypha::Topic{this->topic}));
    auto v2 = hypha::Node().advertise<ReadingV2>(hypha::Topic{this->topic});
    auto v2Reader = hypha::Node().subscribe<ReadingV2>(
        hypha::Topic{this->topic}, [&secondVersion](const hypha::MessagePtr<ReadingV2>& message) {
            secondVersion.push_back(message->value);
        }
    );
    MigratingCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node).has_value());

    ReadingV2 heavier;
    heavier.value = 20;
    heavier.weight = 1.5;
    ASSERT_TRUE(v1->publish(reading(10)));
    ASSERT_TRUE(v2.publish(heavier));
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return cycle.lastValue == 10 && !secondVersion.empty();
    }));
    EXPECT_EQ(secondVersion, std::vector<int>{20});
    EXPECT_EQ(cycle.reading.received(), 1U) << "version 1 reads version 1 alone";
    EXPECT_TRUE(refusedNothing(v2Reader.metrics()));
    ASSERT_TRUE(runUntil(*this->node, [&v2Reader] {
        return v2Reader.metrics().otherSchemas == 1U;
    })) << "the reader of version 2 shows version 1 published beside it";

    ::testing::internal::CaptureStderr();
    v1.reset();
    const auto told = std::chrono::steady_clock::now() + std::chrono::milliseconds(6500);
    while (std::chrono::steady_clock::now() < told) {
        ASSERT_TRUE(v2.publish(heavier));
        ASSERT_TRUE(runSlices(*this->node, 1)) << "the node keeps running";
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }
    const std::string log = ::testing::internal::GetCapturedStderr();
    const std::string line =
        "hypha: topic '" + this->topic
        + "' has publishers, none of type hypha_test/Reading (581e4b1b): published are "
          "hypha_test/Reading (f79835de)";
    EXPECT_NE(log.find(line), std::string::npos) << log;
    EXPECT_EQ(log.find(line), log.rfind(line)) << "said once: " << log;
    EXPECT_EQ(log.find("refused a frame"), std::string::npos) << log;
    EXPECT_EQ(cycle.verdict, hypha::InputVerdict::Stale);

    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    ASSERT_TRUE(state.has_value());
    ASSERT_EQ(state->inputs.size(), 1U);
    EXPECT_EQ(state->inputs[0].md5, "581e4b1bda6a172aa19e1038a38e01eb");
    EXPECT_EQ(state->inputs[0].verdict, hypha_msgs::RequiredInput::STALE);
    EXPECT_EQ(state->inputs[0].rejected, 0U);
    EXPECT_TRUE(state->inputs[0].other_schemas_checked);
    ASSERT_EQ(state->inputs[0].other_schemas.size(), 1U) << "the row names the other version";
    EXPECT_EQ(state->inputs[0].other_schemas[0].data_type, "hypha_test/Reading");
    EXPECT_EQ(state->inputs[0].other_schemas[0].md5, "f79835def461350458ead76f07016dd7");

    auto back = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    ASSERT_TRUE(runUntil(*this->node, [&] {
        static_cast<void>(back.publish(reading(11)));
        return cycle.lastValue == 11;
    }));
    EXPECT_EQ(cycle.verdict, hypha::InputVerdict::Usable);
}

/// A cycle whose Latest of version 1 has no time limit.
class UnlimitedCycle final : public hypha_test::StepCycle
{
  public:
    explicit UnlimitedCycle(const std::string& topic)
        : hypha_test::StepCycle("unlimited_cycle")
        , reading(*this, "reading", hypha::Topic{topic}, hypha::unsupervised)
    {
    }

    hypha::Latest<Reading> reading;
    hypha::InputVerdict    verdict{hypha::InputVerdict::NeverArrived};
    std::optional<int>     lastValue;

    void step(const hypha::CycleTime&) override
    {
        this->verdict = this->reading.verdict();
        if (this->reading.usable()) {
            this->lastValue = (*this->reading).value;
        }
    }
};

// A migration gone wrong: the peer of a node is replaced by one built from version 2. An
// input with no time limit stays usable on the last value of version 1, so its verdict
// tells nothing; the ~/state row and the subscriber counters show that another version is
// what is published now.
TEST_F(OverloadingFixture, APublisherReplacedByAnotherVersionIsShownBesideAnUnlimitedInput)
{
    const std::string unlimitedTopic = testTopic("/unlimited");
    std::vector<int>  plain;
    auto v1 = std::optional(hypha::Node().advertise<Reading>(hypha::Topic{unlimitedTopic}));
    auto plainReader = hypha::Node().subscribe<Reading>(
        hypha::Topic{unlimitedTopic}, [&plain](const hypha::MessagePtr<Reading>& message) {
            plain.push_back(message->value);
        }
    );
    EXPECT_FALSE(plainReader.metrics().otherSchemas.has_value()) << "not checked before a run";
    UnlimitedCycle cycle(unlimitedTopic);
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node).has_value());

    ASSERT_TRUE(v1->publish(reading(7)));
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return cycle.lastValue == 7 && !plain.empty();
    }));
    v1.reset();
    auto      v2 = hypha::Node().advertise<ReadingV2>(hypha::Topic{unlimitedTopic});
    ReadingV2 heavier;
    heavier.value = 8;
    const auto shown = [&plainReader] {
        const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
        return state.has_value() && state->inputs.size() == 1U
            && state->inputs[0].other_schemas.size() == 1U
            && plainReader.metrics().otherSchemas == 1U;
    };
    ASSERT_TRUE(runUntil(*this->node, [&] {
        static_cast<void>(v2.publish(heavier));
        return shown();
    }));

    EXPECT_EQ(cycle.verdict, hypha::InputVerdict::Usable) << "no limit: the old value stays usable";
    EXPECT_EQ(cycle.lastValue, 7);
    EXPECT_EQ(plain, std::vector<int>{7});
    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    ASSERT_TRUE(state.has_value());
    EXPECT_EQ(state->inputs[0].verdict, hypha_msgs::RequiredInput::USABLE);
    EXPECT_EQ(state->inputs[0].md5, "581e4b1bda6a172aa19e1038a38e01eb");
    EXPECT_EQ(state->inputs[0].other_schemas[0].md5, "f79835def461350458ead76f07016dd7");
    EXPECT_TRUE(refusedNothing(plainReader.metrics()));
}
