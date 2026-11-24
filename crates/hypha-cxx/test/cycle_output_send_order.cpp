#include "hypha/cycle_node.h"
#include "hypha/hypha.h"

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_msgs/Float64.h>

#include <chrono>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

using Float64 = std_msgs::Float64;

std::string outputTopic(const std::string& suffix = {})
{
    return hypha_test::testTopic("/cycle_output_snapshot" + suffix);
}

class SnapshotCycle final : public hypha::CycleNode
{
  public:
    hypha::Output<Float64> output{*this, "out"};
    bool                   sent{false};

    void step(const hypha::CycleTime&) override
    {
        if (this->sent) {
            return;
        }
        Float64 message{1.0};
        this->output.send(message);
        // The step has registered the bytes: a later change of the message is not sent.
        message.data = 2.0;
        this->sent = true;
    }
};

class RepeatingCycle final : public hypha::CycleNode
{
  public:
    hypha::Output<Float64> output{*this, "out"};
    uint32_t               steps{0};

    void step(const hypha::CycleTime&) override
    {
        this->steps += 1;
        this->output.send(Float64{static_cast<double>(this->steps)});
    }
};

class RepeatingTripleCycle final : public hypha::CycleNode
{
  public:
    Output<Float64> triple{*this, "triple", perStep(3)};
    uint32_t        steps{0};

    void step(const hypha::CycleTime&) override
    {
        this->steps += 1;
        for (uint32_t index = 1; index <= 3; ++index) {
            this->triple.send(Float64{static_cast<double>(this->steps * 10 + index)});
        }
    }
};

class InterleavedCycle final : public hypha::CycleNode
{
  public:
    Output<Float64>        first{*this, "first", perStep(2)};
    hypha::Output<Float64> second{*this, "second"};
    bool                   done{false};

    void step(const hypha::CycleTime&) override
    {
        if (this->done) {
            return;
        }
        this->first.send(Float64{1.0});
        this->second.send(Float64{2.0});
        this->first.send(Float64{3.0});
        this->done = true;
    }
};

class OverflowCycle final : public hypha::CycleNode
{
  public:
    Output<Float64> output{*this, "out", perStep(2)};

    void step(const hypha::CycleTime&) override
    {
        for (double value : {1.0, 2.0, 3.0}) {
            try {
                this->output.send(Float64{value});
            } catch (const std::runtime_error&) {
                // The callback catches the refusal. The emission must still fail atomically.
            }
        }
    }
};

class DisabledOverflowCycle final : public hypha::CycleNode
{
  public:
    Output<Float64> output{*this, "disabled", optionalTopic};

    void step(const hypha::CycleTime&) override
    {
        for (int call = 0; call < 2; ++call) {
            try {
                this->output.send(Float64{1.0});
            } catch (const std::runtime_error&) {
                // The callback catches the refusal. The limit still fails the step.
            }
        }
    }
};

class IdleCycle final : public hypha::CycleNode
{
  public:
    hypha::Output<Float64> output{*this, "out"};

    void step(const hypha::CycleTime&) override
    {
    }
};

struct ReceiveValues
{
    std::vector<double>* values;

    void operator()(const hypha::Subscriber<Float64>::ConstPtr& message) const
    {
        this->values->push_back(message->data);
    }
};

class CycleOutputSnapshotFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_cycle_output_snapshot.toml";
        std::ofstream output(this->path);
        output << "[ports.outputs.out]\ntopic='" << outputTopic() << "'\n"
               << "[ports.outputs.first]\ntopic='" << outputTopic("_first") << "'\n"
               << "[ports.outputs.second]\ntopic='" << outputTopic("_second") << "'\n"
               << "[ports.outputs.triple]\ntopic='" << outputTopic("_third") << "'\n"
               << "[ports.outputs.disabled]\ntopic=''\n";
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1'000'000'000)));
        this->nodeHandle.emplace("cycle_output_snapshot", this->path.string());
    }

    void TearDown() override
    {
        this->nodeHandle.reset();
        std::filesystem::remove(this->path);
    }

    std::optional<hypha::Node> nodeHandle;
    std::filesystem::path      path;
};

}  // namespace

TEST_F(CycleOutputSnapshotFixture, SendPublishesTheMessageValueAtTheCall)
{
    hypha::Node&        node = *this->nodeHandle;
    std::vector<double> values;
    auto                subscriber =
        hypha::Node().subscribe<Float64>(hypha::Topic{outputTopic()}, ReceiveValues{&values});
    ASSERT_TRUE(subscriber.enabled());

    SnapshotCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(node).has_value());

    // One 10 ms advance crosses exactly one deadline of the 100 Hz cycle. The advance
    // waits for its step, so a slow host cannot make one advance run two steps.
    hypha_test::advance(hypha::Duration::fromMillis(10));
    ASSERT_TRUE(hypha_test::runUntil(node, [&] {
        return !values.empty();
    }));

    ASSERT_EQ(values.size(), 1U);
    EXPECT_EQ(values.front(), 1.0);
}

TEST_F(CycleOutputSnapshotFixture, DefaultLimitResetsForEachStep)
{
    hypha::Node&        node = *this->nodeHandle;
    std::vector<double> values;
    auto                subscriber =
        hypha::Node().subscribe<Float64>(hypha::Topic{outputTopic()}, ReceiveValues{&values});

    RepeatingCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(node).has_value());

    // One 10 ms advance crosses exactly one deadline of the 100 Hz cycle. Each advance
    // waits for its step, so a slow host cannot make one advance run two steps.
    for (std::size_t step = 1; step <= 2; ++step) {
        hypha_test::advance(hypha::Duration::fromMillis(10));
        ASSERT_TRUE(
            hypha_test::runUntil(
                node,
                [&] {
                    return values.size() >= step;
                }
            )
        ) << "step "
          << step << " never ran";
    }

    ASSERT_EQ(values.size(), 2U);
    EXPECT_EQ(values[0], 1.0);
    EXPECT_EQ(values[1], 2.0);
    EXPECT_EQ(cycle.output.sentCount(), 2U);
}

TEST_F(CycleOutputSnapshotFixture, ThreeMessagesPerStepArriveInSendOrder)
{
    hypha::Node&        node = *this->nodeHandle;
    std::vector<double> values;
    auto                subscriber = hypha::Node().subscribe<Float64>(
        hypha::Topic{outputTopic("_third")}, ReceiveValues{&values}
    );

    RepeatingTripleCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(node).has_value());

    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (values.size() < 6U && std::chrono::steady_clock::now() < deadline) {
        hypha_test::advance(hypha::Duration::fromMillis(10));
        ASSERT_TRUE(node.runFor(hypha::Duration::fromMillis(1)).ok());
    }

    // Each step holds three loans until it ends, so all three messages of a step
    // reach the subscriber, in the order the step sent them.
    ASSERT_GE(values.size(), 6U);
    const std::vector<double> firstTwoSteps(values.begin(), values.begin() + 6);
    EXPECT_EQ(firstTwoSteps, (std::vector<double>{11.0, 12.0, 13.0, 21.0, 22.0, 23.0}));
    EXPECT_EQ(cycle.triple.sentCount(), cycle.steps * 3U);
}

TEST_F(CycleOutputSnapshotFixture, EachOutputKeepsSendOrderAndMessageSnapshots)
{
    hypha::Node&        node = *this->nodeHandle;
    std::vector<double> firstValues;
    std::vector<double> secondValues;
    auto                firstSubscriber = hypha::Node().subscribe<Float64>(
        hypha::Topic{outputTopic("_first")}, ReceiveValues{&firstValues}
    );
    auto secondSubscriber = hypha::Node().subscribe<Float64>(
        hypha::Topic{outputTopic("_second")}, ReceiveValues{&secondValues}
    );

    InterleavedCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(node).has_value());

    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while ((firstValues.size() < 2U || secondValues.empty())
           && std::chrono::steady_clock::now() < deadline) {
        hypha_test::advance(hypha::Duration::fromMillis(10));
        ASSERT_TRUE(node.runFor(hypha::Duration::fromMillis(1)).ok());
    }

    ASSERT_EQ(firstValues.size(), 2U);
    ASSERT_EQ(secondValues.size(), 1U);
    EXPECT_EQ(firstValues[0], 1.0);
    EXPECT_EQ(firstValues[1], 3.0);
    EXPECT_EQ(secondValues[0], 2.0);
    EXPECT_EQ(cycle.first.sentCount(), 2U);
    EXPECT_EQ(cycle.second.sentCount(), 1U);
}

TEST_F(CycleOutputSnapshotFixture, OverflowAfterAcceptedSendsFailsTheWholeStep)
{
    hypha::Node&  node = *this->nodeHandle;
    OverflowCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(node).has_value());

    hypha::RunResult run = node.runFor(hypha::Duration::fromMillis(1));
    const auto       deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (run.ok() && std::chrono::steady_clock::now() < deadline) {
        hypha_test::advance(hypha::Duration::fromMillis(10));
        run = node.runFor(hypha::Duration::fromMillis(1));
    }

    EXPECT_FALSE(run.ok());
    EXPECT_EQ(run.status(), hypha::Status::ResourceLimit);
    EXPECT_EQ(cycle.output.sentCount(), 2U);
    const auto health = cycle.health();
    ASSERT_TRUE(health.has_value());
    EXPECT_EQ(health->output_attempts, 0U);
}

TEST_F(CycleOutputSnapshotFixture, DisabledOutputStillEnforcesItsPerStepLimit)
{
    hypha::Node&          node = *this->nodeHandle;
    DisabledOverflowCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(node).has_value());

    hypha::RunResult run = node.runFor(hypha::Duration::fromMillis(1));
    const auto       deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (run.ok() && std::chrono::steady_clock::now() < deadline) {
        hypha_test::advance(hypha::Duration::fromMillis(10));
        run = node.runFor(hypha::Duration::fromMillis(1));
    }

    EXPECT_FALSE(run.ok());
    EXPECT_EQ(run.status(), hypha::Status::ResourceLimit);
    EXPECT_EQ(cycle.output.sentCount(), 1U);
    const auto health = cycle.health();
    ASSERT_TRUE(health.has_value());
    EXPECT_EQ(health->output_attempts, 0U);
}

TEST_F(CycleOutputSnapshotFixture, SendRequiresAnActiveStepCallback)
{
    IdleCycle cycle;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->nodeHandle).has_value());

    EXPECT_THROW(cycle.output.send(Float64{1.0}), std::logic_error);
}
