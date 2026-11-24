// A node reads its own execution health through the node.* metrics.

#include <hypha_msgs/NodeState.h>

#include "test_types.h"

#include <gtest/gtest.h>

#include <chrono>
#include <cstdint>
#include <expected>
#include <optional>
#include <string>
#include <thread>

using hypha_test::Reading;
using hypha_test::testTopic;
using namespace hypha::literals;

namespace {

constexpr int64_t TEST_CLOCK_START_NANOS = 1000000000;

class NodeHealthFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(TEST_CLOCK_START_NANOS)));
        this->node.emplace("node_health");
    }

    std::optional<hypha::Node> node;
};

struct CountingReader
{
    int* calls;

    void operator()(const hypha::MessagePtr<Reading>&) const
    {
        *this->calls += 1;
    }
};

TEST_F(NodeHealthFixture, ANewNodeReportsNothingYet)
{
    EXPECT_EQ(hypha::metric("node.callbacks_invoked"), 0U);
    EXPECT_EQ(hypha::metric("node.callback_failures"), 0U);
    EXPECT_EQ(hypha::metric("node.last_callback_status"), 0U);
    EXPECT_EQ(hypha::metric("node.slow_callbacks"), 0U);
    EXPECT_EQ(hypha::metric("node.last_sweep_end_nanos"), 0U);
}

TEST_F(NodeHealthFixture, ACleanRunReportsNoFailureAndTheSweepEndInFrameworkTime)
{
    const std::string topic = testTopic("/counting_reader");
    int               calls = 0;
    auto              publisher = hypha::Node().advertise<Reading>(hypha::Topic{topic});
    auto subscriber = hypha::Node().subscribe<Reading>(hypha::Topic{topic}, CountingReader{&calls});
    ASSERT_TRUE(publisher.publish(Reading{1, "one"}));

    ASSERT_TRUE(node->runFor(20_ms).ok());

    ASSERT_EQ(calls, 1);
    EXPECT_EQ(hypha::metric("node.callbacks_invoked"), 1U);
    EXPECT_EQ(hypha::metric("node.callback_failures"), 0U);
    EXPECT_EQ(hypha::metric("node.last_callback_status"), 0U);
    EXPECT_EQ(hypha::metric("node.is_sim_time"), 1U) << "the test clock drives framework time";
    EXPECT_EQ(
        hypha::metric("node.last_sweep_end_nanos"), static_cast<uint64_t>(TEST_CLOCK_START_NANOS)
    );
}

TEST_F(NodeHealthFixture, NodeMetricsAreAbsentWithoutANode)
{
    this->node.reset();

    EXPECT_EQ(hypha::metric("node.callbacks_invoked"), std::nullopt);
    EXPECT_TRUE(hypha::metric("log.lines_written").has_value())
        << "the process-wide log counters exist without a node";
}

}  // namespace

namespace {

/// A rate cycle that counts its steps and sleeps @p sleepFirst in its first one.
class TimedCycle final : public hypha_test::StepCycle
{
  public:
    TimedCycle(std::string name, std::chrono::milliseconds sleepFirst)
        : hypha_test::StepCycle(std::move(name)), sleepFirst_(sleepFirst)
    {
    }

    int steps{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        if (this->steps == 1) {
            std::this_thread::sleep_for(this->sleepFirst_);
        }
    }

  private:
    std::chrono::milliseconds sleepFirst_;
};

hypha_msgs::NodeState stateNow()
{
    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    EXPECT_TRUE(state.has_value());
    return state.value_or(hypha_msgs::NodeState{});
}

}  // namespace

TEST(NodeStateCycles, OneRowPerCycleInCreationOrderCountsStepsAndMeasuresASlowStep)
{
    ASSERT_TRUE(hypha::disableSimTime());
    hypha::Node node("node_state_cycles");
    TimedCycle  slow("slow", std::chrono::milliseconds(15));
    TimedCycle  fast("fast", std::chrono::milliseconds(0));
    slow.setRate(hypha::Hz(100.0));
    fast.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(slow.start(node).has_value());
    ASSERT_TRUE(fast.start(node).has_value());

    ASSERT_TRUE(node.runFor(100_ms).ok());

    const hypha_msgs::NodeState state = stateNow();
    ASSERT_EQ(state.cycles.size(), 2U);
    EXPECT_EQ(state.cycles[0].name, "slow");
    EXPECT_EQ(state.cycles[1].name, "fast");
    const hypha_msgs::CycleState& row = state.cycles[0];
    EXPECT_EQ(row.steps, static_cast<uint64_t>(slow.steps));
    EXPECT_GE(row.max_step_nanos, 15'000'000U) << "the first step slept 15 ms";
    EXPECT_GE(row.max_step_nanos, row.last_step_nanos);
    EXPECT_GE(row.overruns, 1U) << "the 15 ms step is longer than the 10 ms period";
    EXPECT_FALSE(state.sim_time);
    EXPECT_EQ(state.state, hypha_msgs::NodeState::INPUTS_USABLE)
        << "a missed period or an overrun does not change the state";
}

TEST(NodeStateCycles, ASlowStepUnderSimulatedTimeCountsNoOverrun)
{
    ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(TEST_CLOCK_START_NANOS)));
    hypha::Node node("node_state_sim_cycles");
    TimedCycle  slow("slow", std::chrono::milliseconds(15));
    slow.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(slow.start(node).has_value());
    for (int slice = 0; slice < 5; ++slice) {
        hypha_test::advance(10_ms);
        ASSERT_TRUE(node.runFor(1_ms).ok());
    }

    const hypha_msgs::NodeState state = stateNow();
    ASSERT_EQ(state.cycles.size(), 1U);
    EXPECT_GE(state.cycles[0].steps, 1U);
    EXPECT_GE(state.cycles[0].max_step_nanos, 15'000'000U);
    EXPECT_EQ(state.cycles[0].overruns, 0U);
    EXPECT_TRUE(state.sim_time);
}
