// A cycle that runs on the samples of its Arrivals trigger ports instead of a rate, with
// a fallback step when the triggers fall silent. Host time.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"
#include <hypha_msgs/NodeState.h>

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_msgs/Float64.h>

#include <algorithm>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <thread>
#include <utility>
#include <vector>

using Float64 = std_msgs::Float64;
using namespace hypha::literals;

namespace {

std::string topicFor(const std::string& name)
{
    return hypha_test::testTopic("/" + name);
}

/// One step as the cycle saw it.
struct Seen
{
    size_t samples{0};
    bool   stale{false};
};

/// One port table of the parameter file: its name, and its `silence_ms` when set.
struct PortTable
{
    std::string        port;
    std::optional<int> silenceMs;
};

/// Writes a parameter file for one node per test, with one table per port.
class TriggeredFixture : public ::testing::Test
{
  protected:
    void init(const std::string& port, std::optional<int> silenceMs)
    {
        this->init({PortTable{port, silenceMs}});
    }

    void init(const std::vector<PortTable>& tables)
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_triggered_cycle_test.toml";
        std::ofstream output(this->path);
        for (const PortTable& table : tables) {
            output << "[ports.inputs." << table.port << "]\ntopic='" << topicFor(table.port)
                   << "'\n";
            if (table.silenceMs.has_value()) {
                output << "silence_ms = " << *table.silenceMs << "\n";
            }
        }
        output.close();
        this->node.emplace("triggered_cycle_test", this->path.string());
    }

    hypha::Publisher<Float64> publisherOf(const std::string& port)
    {
        return hypha::Node().advertise<Float64>(hypha::Topic{topicFor(port)});
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

class OnSample final : public hypha::CycleNode
{
  public:
    explicit OnSample(bool supervised) : trigger(*this, "trig", unsupervised(!supervised))
    {
        this->setTrigger(this->trigger);
    }

    hypha::Arrivals<Float64> trigger;
    std::vector<Seen>        seen;

    void step(const hypha::CycleTime&) override
    {
        this->seen.push_back(Seen{this->trigger.samples().size(), this->trigger.stale()});
    }
};

/// Runs the node in 1 ms slices until @p done holds, or a real-time bound passes.
template <typename Done>
bool runUntil(hypha::Node& node, Done done)
{
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    while (!done()) {
        if (std::chrono::steady_clock::now() >= deadline) {
            return false;
        }
        if (!node.runFor(1_ms).ok()) {
            return false;
        }
    }
    return true;
}

}  // namespace

TEST_F(TriggeredFixture, RunsOneStepPerSample)
{
    this->init("trig", 10000);
    OnSample   cycle(true);
    const auto started = cycle.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    auto publisher = hypha::Node().advertise<Float64>(hypha::Topic{topicFor("trig")});
    for (int value = 1; value <= 3; ++value) {
        ASSERT_TRUE(publisher.publish(Float64{static_cast<double>(value)}));
    }
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return cycle.seen.size() >= 3;
    }));
    ASSERT_TRUE(this->node->runFor(20_ms).ok());
    ASSERT_EQ(cycle.seen.size(), 3U);
    for (const Seen& step : cycle.seen) {
        EXPECT_EQ(step.samples, 1U);
    }
}

TEST_F(TriggeredFixture, FallsBackBeforeTheFirstTrigger)
{
    this->init("trig", 50);
    OnSample cycle(true);
    ASSERT_TRUE(cycle.start(*this->node));
    ASSERT_TRUE(this->node->runFor(130_ms).ok());
    ASSERT_FALSE(cycle.seen.empty());
    EXPECT_EQ(cycle.seen.front().samples, 0U);
    EXPECT_TRUE(cycle.seen.front().stale);
}

struct NoStep
{
    void operator()(const hypha::CycleTime&) const
    {
    }
};

TEST_F(TriggeredFixture, NeitherARateNorATriggerIsRefused)
{
    this->init("trig", 50);
    hypha::Cycle cycle("no_schedule");
    // The library refuses it at commit; the error carries its reason.
    const auto started = cycle.start(*this->node, NoStep{});
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(started.error().find("cycle 'no_schedule'"), std::string::npos) << started.error();
    EXPECT_NE(started.error().find("it has neither a rate nor a trigger"), std::string::npos)
        << started.error();
}

TEST_F(TriggeredFixture, ARateAndATriggerAreRefusedWithTheReason)
{
    this->init("trig", 50);
    OnSample cycle(true);
    cycle.setRate(hypha::Hz(10.0));
    const auto started = cycle.start(*this->node);
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(started.error().find("cycle 'triggered_cycle_test'"), std::string::npos)
        << started.error();
    EXPECT_NE(started.error().find("a rate and a trigger exclude each other"), std::string::npos)
        << started.error();
}

class LatestTrigger final : public hypha::CycleNode
{
  public:
    LatestTrigger()
    {
        this->setTrigger(this->trigger);
    }

    hypha::Latest<Float64> trigger{*this, "trig"};

    void step(const hypha::CycleTime&) override
    {
    }
};

TEST_F(TriggeredFixture, ALatestPortCannotBeTheTrigger)
{
    this->init("trig", 100);
    LatestTrigger cycle;
    const auto    started = cycle.start(*this->node);
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(started.error().find("Arrivals"), std::string::npos) << started.error();
}

namespace {

/// The `~/state` of the node now.
hypha_msgs::NodeState stateNow()
{
    const auto state = hypha::nodeState();
    EXPECT_TRUE(state.has_value());
    return state.value_or(hypha_msgs::NodeState{});
}

/// An untimed trigger runs no fallback: no step before its first sample, one per sample.
/// While it never arrived the node waits for it.
void expectNoFallbackAndOneStepPerSample(hypha::Node& node, OnSample& cycle)
{
    const auto started = cycle.start(node);
    ASSERT_TRUE(started.has_value()) << started.error();
    EXPECT_TRUE(cycle.trigger.untimed());
    ASSERT_TRUE(node.runFor(300_ms).ok());
    EXPECT_TRUE(cycle.seen.empty()) << "no fallback without a limit";
    const hypha_msgs::NodeState waiting = stateNow();
    EXPECT_EQ(waiting.state, hypha_msgs::NodeState::WAITING) << "it waits for its trigger";
    ASSERT_EQ(waiting.inputs.size(), 1U);
    EXPECT_EQ(waiting.inputs[0].verdict, hypha_msgs::RequiredInput::NEVER_ARRIVED);
    auto publisher = hypha::Node().advertise<Float64>(hypha::Topic{topicFor("trig")});
    ASSERT_TRUE(publisher.publish(Float64{1.0}));
    ASSERT_TRUE(runUntil(node, [&] {
        return !cycle.seen.empty();
    }));
    ASSERT_TRUE(node.runFor(300_ms).ok());
    ASSERT_EQ(cycle.seen.size(), 1U);
    EXPECT_EQ(cycle.seen[0].samples, 1U);
    EXPECT_FALSE(cycle.seen[0].stale);
    EXPECT_EQ(stateNow().state, hypha_msgs::NodeState::INPUTS_USABLE);
}

}  // namespace

TEST_F(TriggeredFixture, ATriggerWithSilenceMsZeroRunsNoFallback)
{
    this->init("trig", 0);
    OnSample cycle(true);
    expectNoFallbackAndOneStepPerSample(*this->node, cycle);
}

TEST_F(TriggeredFixture, AnUnsupervisedTriggerRunsNoFallback)
{
    this->init("trig", std::nullopt);
    OnSample cycle(false);
    expectNoFallbackAndOneStepPerSample(*this->node, cycle);
}

TEST_F(TriggeredFixture, SetTriggerAfterStartThrows)
{
    this->init("trig", 50);
    OnSample cycle(true);
    ASSERT_TRUE(cycle.start(*this->node));
    EXPECT_THROW(cycle.setTrigger(cycle.trigger), std::logic_error);
}

namespace {

/// One step of a cycle with two triggers, as it saw them.
struct SeenTwo
{
    size_t a{0};
    size_t b{0};
    bool   aStale{false};
    bool   bStale{false};
};

/// A cycle that steps on either of two Arrivals ports; `a` is constructed first.
class OnEither final : public hypha::CycleNode
{
  public:
    OnEither()
    {
        // The order of the calls has no meaning; the ports keep construction order.
        this->setTrigger(this->b);
        this->setTrigger(this->a);
    }

    hypha::Arrivals<Float64> a{*this, "a"};
    hypha::Arrivals<Float64> b{*this, "b"};
    std::vector<SeenTwo>     seen;

    void step(const hypha::CycleTime&) override
    {
        this->seen.push_back(
            SeenTwo{
                this->a.samples().size(), this->b.samples().size(), this->a.stale(), this->b.stale()
            }
        );
    }
};

}  // namespace

TEST_F(TriggeredFixture, TwoTriggersRunOneStepPerSampleEach)
{
    this->init({PortTable{"a", 10000}, PortTable{"b", 5000}});
    OnEither   cycle;
    const auto started = cycle.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    auto a = this->publisherOf("a");
    auto b = this->publisherOf("b");
    ASSERT_TRUE(a.publish(Float64{1.0}));
    ASSERT_TRUE(b.publish(Float64{2.0}));
    ASSERT_TRUE(a.publish(Float64{3.0}));
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return cycle.seen.size() >= 3;
    }));
    ASSERT_TRUE(this->node->runFor(20_ms).ok());
    ASSERT_EQ(cycle.seen.size(), 3U);
    const std::vector<std::pair<size_t, size_t>> expected{{1, 0}, {0, 1}, {1, 0}};
    for (size_t index = 0; index < expected.size(); ++index) {
        EXPECT_EQ(cycle.seen[index].a, expected[index].first) << index;
        EXPECT_EQ(cycle.seen[index].b, expected[index].second) << index;
    }
}

TEST_F(TriggeredFixture, OnlyTheFiringPortHoldsItsSample)
{
    this->init({PortTable{"a", 10000}, PortTable{"b", 5000}});
    OnEither cycle;
    ASSERT_TRUE(cycle.start(*this->node));
    auto a = this->publisherOf("a");
    auto b = this->publisherOf("b");
    for (int round = 0; round < 3; ++round) {
        ASSERT_TRUE(b.publish(Float64{1.0}));
        ASSERT_TRUE(a.publish(Float64{2.0}));
    }
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return cycle.seen.size() >= 6;
    }));
    for (const SeenTwo& step : cycle.seen) {
        EXPECT_EQ(step.a + step.b, 1U);
    }
}

TEST_F(TriggeredFixture, ASilentTriggerReadsStaleWhileTheOtherFires)
{
    this->init({PortTable{"a", 50}, PortTable{"b", 10000}});
    OnEither cycle;
    ASSERT_TRUE(cycle.start(*this->node));
    ASSERT_TRUE(this->node->runFor(80_ms).ok());
    auto b = this->publisherOf("b");
    ASSERT_TRUE(b.publish(Float64{1.0}));
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return std::ranges::any_of(cycle.seen, [](const SeenTwo& step) {
            return step.b == 1;
        });
    }));
    const auto fired = std::ranges::find_if(cycle.seen, [](const SeenTwo& step) {
        return step.b == 1;
    });
    EXPECT_EQ(fired->a, 0U);
    EXPECT_TRUE(fired->aStale) << "a has sent nothing for its 50 ms";
    EXPECT_FALSE(fired->bStale);
}

TEST_F(TriggeredFixture, FallsBackAtTheShortestSilence)
{
    this->init({PortTable{"a", 10000}, PortTable{"b", 50}});
    OnEither cycle;
    ASSERT_TRUE(cycle.start(*this->node));
    ASSERT_TRUE(this->node->runFor(130_ms).ok());
    ASSERT_FALSE(cycle.seen.empty()) << "a fallback runs after b's 50 ms";
    for (const SeenTwo& step : cycle.seen) {
        EXPECT_EQ(step.a + step.b, 0U);
        EXPECT_FALSE(step.aStale) << "a is within its own limit";
        EXPECT_TRUE(step.bStale);
    }
}

namespace {

/// A cycle with two triggers; `b` runs without a silence limit.
class OneTriggerUnlimited final : public hypha::CycleNode
{
  public:
    OneTriggerUnlimited()
    {
        this->setTrigger(this->a);
        this->setTrigger(this->b);
    }

    hypha::Arrivals<Float64> a{*this, "a"};
    hypha::Arrivals<Float64> b{*this, "b", unsupervised};
    std::vector<SeenTwo>     seen;

    void step(const hypha::CycleTime&) override
    {
        this->seen.push_back(
            SeenTwo{
                this->a.samples().size(), this->b.samples().size(), this->a.stale(), this->b.stale()
            }
        );
    }
};

}  // namespace

TEST_F(TriggeredFixture, TheFallbackCountsOnlyTheTimedTriggers)
{
    this->init({PortTable{"a", 100}, PortTable{"b", std::nullopt}});
    OneTriggerUnlimited cycle;
    const auto          started = cycle.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    ASSERT_TRUE(this->node->runFor(250_ms).ok());
    ASSERT_FALSE(cycle.seen.empty()) << "a fallback runs after a's 100 ms";
    for (const SeenTwo& step : cycle.seen) {
        EXPECT_EQ(step.a + step.b, 0U);
        EXPECT_TRUE(step.aStale);
        EXPECT_FALSE(step.bStale) << "an untimed trigger is never stale";
    }
}

namespace {

class SameTriggerTwice final : public hypha::CycleNode
{
  public:
    SameTriggerTwice()
    {
        this->setTrigger(this->trigger);
        this->setTrigger(this->trigger);
    }

    hypha::Arrivals<Float64> trigger{*this, "trig"};
    size_t                   steps{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
    }
};

}  // namespace

TEST_F(TriggeredFixture, SetTriggerTwiceIsOneTrigger)
{
    this->init("trig", 10000);
    SameTriggerTwice cycle;
    ASSERT_TRUE(cycle.start(*this->node));
    auto publisher = this->publisherOf("trig");
    ASSERT_TRUE(publisher.publish(Float64{1.0}));
    ASSERT_TRUE(publisher.publish(Float64{2.0}));
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return cycle.steps >= 2;
    }));
    ASSERT_TRUE(this->node->runFor(20_ms).ok());
    EXPECT_EQ(cycle.steps, 2U);
}

namespace {

class SharedTriggers final : public hypha::CycleNode
{
  public:
    SharedTriggers()
    {
        this->setTrigger(this->first);
        this->setTrigger(this->second);
    }

    hypha::Arrivals<Float64> first{*this, "first"};
    hypha::Arrivals<Float64> second{*this, "second", hypha::sharing(this->first)};

    void step(const hypha::CycleTime&) override
    {
    }
};

}  // namespace

TEST_F(TriggeredFixture, TwoTriggersOnOneSharedHistoryAreRefused)
{
    this->init({PortTable{"first", 100}, PortTable{"second", 100}});
    SharedTriggers cycle;
    const auto     started = cycle.start(*this->node);
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(started.error().find("'second'"), std::string::npos) << started.error();
    EXPECT_NE(started.error().find("InvalidArgument"), std::string::npos) << started.error();
}

namespace {

/// A cycle whose first trigger port is destroyed before start().
class RemovedTrigger final : public hypha::CycleNode
{
  public:
    RemovedTrigger() : removed(std::make_unique<hypha::Arrivals<Float64>>(*this, "a"))
    {
        this->setTrigger(*this->removed);
        this->setTrigger(this->kept);
    }

    std::unique_ptr<hypha::Arrivals<Float64>> removed;
    hypha::Arrivals<Float64>                  kept{*this, "b"};
    size_t                                    steps{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
    }
};

}  // namespace

TEST_F(TriggeredFixture, ARemovedTriggerPortIsDropped)
{
    this->init({PortTable{"a", 10000}, PortTable{"b", 10000}});
    RemovedTrigger cycle;
    cycle.removed.reset();
    const auto started = cycle.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    auto b = this->publisherOf("b");
    ASSERT_TRUE(b.publish(Float64{1.0}));
    ASSERT_TRUE(runUntil(*this->node, [&] {
        return cycle.steps >= 1;
    }));
}

namespace {

class UsableArrivals final : public hypha::CycleNode
{
  public:
    hypha::Arrivals<Float64>                          port{*this, "trig"};
    std::vector<std::pair<bool, hypha::InputVerdict>> seen;

    void step(const hypha::CycleTime&) override
    {
        this->seen.emplace_back(this->port.usable(), this->port.verdict());
    }
};

}  // namespace

TEST_F(TriggeredFixture, ArrivalsUsableMatchesVerdict)
{
    this->init("trig", 100);
    UsableArrivals              cycle;
    hypha::testing::StepHarness harness{cycle};
    harness.setNeverArrived(cycle.port);
    harness.step(cycle);
    harness.set(cycle.port, std::vector<Float64>{Float64{1.0}});
    harness.step(cycle);
    harness.setStale(cycle.port);
    harness.step(cycle);
    const std::vector<std::pair<bool, hypha::InputVerdict>> expected{
        {false, hypha::InputVerdict::NeverArrived},
        {true, hypha::InputVerdict::Usable},
        {false, hypha::InputVerdict::Stale},
    };
    EXPECT_EQ(cycle.seen, expected);
}

TEST_F(TriggeredFixture, ForeignTriggerStillThrows)
{
    this->init("trig", 100);
    OnSample     owner(true);
    hypha::Cycle other("other");
    EXPECT_THROW(other.setTrigger(owner.trigger), std::invalid_argument);
}
