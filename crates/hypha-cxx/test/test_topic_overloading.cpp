// Topic name overloading: one topic name carries more than one message type, and each type
// is its own stream (#209). The transport keys a topic by name alone; a frame carries the
// hash of its schema, and a typed reader keeps the frames of its own type. These tests
// observe what a user sees: delivery, the verdicts of cycle inputs, the outputs, a wildcard,
// and what the library counts and says about the frames of the other type.

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

using hypha_test::Command;
using hypha_test::Reading;
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
    JointCycle() : hypha_test::StepCycle("joint_cycle"), latest(*this, "joint"), arrivals(*this, "joint")
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

TEST_F(OverloadingFixture, ASubscriberOfAnotherTypeThanTheOnlyPublisherReceivesNothingAndWarnsOfRefusedFrames)
{
    std::vector<double> commands;
    auto                readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    ASSERT_TRUE(readingPublisher.enabled());

    ::testing::internal::CaptureStderr();
    auto commandSubscriber =
        hypha::Node().subscribe<Command>(hypha::Topic{this->topic}, KeepCommands{&commands});
    const std::string atCreation = ::testing::internal::GetCapturedStderr();
    ASSERT_TRUE(commandSubscriber.enabled());
    EXPECT_EQ(atCreation, "") << "a topic with a publisher of another type is a topic with a publisher";

    ::testing::internal::CaptureStderr();
    for (int index = 1; index <= 5; ++index) {
        ASSERT_TRUE(readingPublisher.publish(reading(index)));
    }
    ASSERT_TRUE(runSlices(*this->node, 20));
    const std::string afterwards = ::testing::internal::GetCapturedStderr();

    EXPECT_TRUE(commands.empty());
    EXPECT_EQ(commandSubscriber.messageCount(), 0U);
    EXPECT_FALSE(commandSubscriber.latestSample().has_value());
    // The only sign is the warning about refused frames: it names the topic and says
    // "schema mismatch", not the two types, and it is the same line a wrong type gives.
    EXPECT_NE(afterwards.find("[WARN]"), std::string::npos) << afterwards;
    EXPECT_NE(afterwards.find("topic '" + this->topic + "' refused a frame: schema mismatch"), std::string::npos)
        << afterwards;
    EXPECT_EQ(commandSubscriber.metrics().schemaMismatch, 5U);
}

TEST_F(OverloadingFixture, ALatestAndAnArrivalsOfTwoTypesOnOnePortNameAreEachUsable)
{
    auto readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
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
    auto readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
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
    EXPECT_FALSE(cycle.inputsOk()) << "the input that never arrived makes the cycle's inputs not ok";

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

TEST_F(OverloadingFixture, TheStateRowOfTheTopicTakesTheWorstVerdictOfItsTypes)
{
    auto commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    JointCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node).has_value());

    for (int slice = 0; slice < 40; ++slice) {
        ASSERT_TRUE(commandPublisher.publish(command(1.0)));
        ASSERT_TRUE(runSlices(*this->node, 1));
    }
    ASSERT_EQ(cycle.arrivalsVerdict, hypha::InputVerdict::Usable);
    ASSERT_EQ(cycle.latestVerdict, hypha::InputVerdict::NeverArrived);

    // `~/state` has one row for each topic: the Latest that never arrived is the verdict of
    // the row, and the node waits, though the Arrivals of the same name is usable.
    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    ASSERT_TRUE(state.has_value());
    EXPECT_EQ(state->state, hypha_msgs::NodeState::WAITING);
    ASSERT_EQ(state->inputs.size(), 1U);
    EXPECT_EQ(state->inputs[0].topic, this->topic);
    EXPECT_EQ(state->inputs[0].verdict, hypha_msgs::RequiredInput::NEVER_ARRIVED);
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
    auto                readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto                commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});

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
    auto                readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto                commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    auto                readingSubscriber =
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

// What the library counts for the frames of the other type. Today a subscriber reads a
// frame of another schema as a refused frame: it counts `schemaMismatch` for every one, and
// the cycle's `~/state` row counts them as `rejected`. These two tests pin that accounting,
// so a change to it is a change of these tests.
TEST_F(OverloadingFixture, TheFramesOfTheOtherTypeAreCountedAsSchemaMismatchOfEachSubscriber)
{
    std::vector<int>    readings;
    std::vector<double> commands;
    auto                readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto                commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
    auto                readingSubscriber =
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
    EXPECT_EQ(ofReadings.schemaMismatch, 5U) << "the five Commands";
    EXPECT_EQ(ofCommands.schemaMismatch, 3U) << "the three Readings";
    EXPECT_EQ(ofReadings.sequenceGap, 0U);
    EXPECT_EQ(ofCommands.sequenceGap, 0U);
    EXPECT_EQ(readingSubscriber.missingSequences(), 0U);
    EXPECT_EQ(commandSubscriber.missingSequences(), 0U);
}

TEST_F(OverloadingFixture, TheFramesOfTheOtherTypeAreRejectedFramesOfTheStateRow)
{
    auto readingPublisher = hypha::Node().advertise<Reading>(hypha::Topic{this->topic});
    auto commandPublisher = hypha::Node().advertise<Command>(hypha::Topic{this->topic});
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
    ASSERT_EQ(state->inputs.size(), 1U);
    EXPECT_EQ(state->inputs[0].verdict, hypha_msgs::RequiredInput::USABLE);
    EXPECT_EQ(state->inputs[0].rejected, 4U) << "the Latest refused the four Commands";
}
