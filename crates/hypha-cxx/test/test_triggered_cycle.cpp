// A cycle that runs on the samples of one Arrivals port instead of a rate, with a
// fallback step when the trigger falls silent. Host time.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include <hypha_msgs/std_msgs/Float64.h>

#include "test_types.h"

#include <gtest/gtest.h>

#include <chrono>
#include <filesystem>
#include <fstream>
#include <optional>
#include <stdexcept>
#include <string>
#include <thread>
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

/// Writes a parameter file for one node per test; @p silence adds `silence_ms`.
class TriggeredFixture : public ::testing::Test
{
  protected:
    void init(const std::string& port, std::optional<int> silenceMs)
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_triggered_cycle_test.toml";
        std::ofstream output(this->path);
        output << "[ports.inputs." << port << "]\ntopic='" << topicFor(port) << "'\n";
        if (silenceMs.has_value()) {
            output << "silence_ms = " << *silenceMs << "\n";
        }
        output.close();
        this->node.emplace("triggered_cycle_test", this->path.string());
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

TEST_F(TriggeredFixture, ATriggerWithoutSilenceIsRefused)
{
    this->init("trig", std::nullopt);
    OnSample   cycle(false);
    const auto started = cycle.start(*this->node);
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(started.error().find("InvalidArgument"), std::string::npos) << started.error();
}

TEST_F(TriggeredFixture, SetTriggerAfterStartThrows)
{
    this->init("trig", 50);
    OnSample cycle(true);
    ASSERT_TRUE(cycle.start(*this->node));
    EXPECT_THROW(cycle.setTrigger(cycle.trigger), std::logic_error);
}
