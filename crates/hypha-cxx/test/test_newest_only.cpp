// A trigger declared newestOnly (#210): ROS's subscriber queue of one, chosen by the reader
// alone. The publisher keeps the default policy; nothing about the topic changes. Host time.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_msgs/Float64.h>

#include <chrono>
#include <filesystem>
#include <fstream>
#include <optional>
#include <string>
#include <vector>

using Float64 = std_msgs::Float64;
using namespace hypha::literals;

namespace {

std::string topicFor(const std::string& name)
{
    return hypha_test::testTopic("/" + name);
}

/// One trigger port, `cmd`, queued or newest only; the step records each value it holds.
class Mux final : public hypha::CycleNode
{
  public:
    explicit Mux(bool newest) : cmd(*this, "cmd", hypha::newestOnly(newest))
    {
        this->setTrigger(this->cmd);
    }

    hypha::Arrivals<Float64> cmd;
    std::vector<double>      values;

    void step(const hypha::CycleTime&) override
    {
        for (const Float64& sample : this->cmd.samples()) {
            this->values.push_back(sample.data);
        }
    }
};

class NewestOnly : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_newest_only_test.toml";
        std::ofstream output(this->path);
        output << "[ports.inputs.cmd]\ntopic='" << topicFor("cmd") << "'\nsilence_ms = 10000\n";
        output.close();
        this->node.emplace("newest_only_test", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    /// Publish 1, 2 and 3 before the node runs, the backlog a slow step leaves, then run
    /// until @p steps values were seen and 20 ms more.
    std::vector<double> backlogOf(Mux& mux, size_t steps)
    {
        auto publisher = hypha::Node().advertise<Float64>(hypha::Topic{topicFor("cmd")});
        for (int value = 1; value <= 3; ++value) {
            EXPECT_TRUE(publisher.publish(Float64{static_cast<double>(value)}));
        }
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
        while (mux.values.size() < steps && std::chrono::steady_clock::now() < deadline) {
            EXPECT_TRUE(this->node->runFor(1_ms).ok());
        }
        EXPECT_TRUE(this->node->runFor(20_ms).ok());
        return mux.values;
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

}  // namespace

TEST_F(NewestOnly, AQueuedTriggerStepsOnEachSampleOfABacklog)
{
    Mux        mux(false);
    const auto started = mux.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    EXPECT_EQ(this->backlogOf(mux, 3), (std::vector<double>{1.0, 2.0, 3.0}));
    const auto health = mux.cmd.bindingHealth();
    ASSERT_TRUE(health.has_value());
    EXPECT_EQ(health->work_limit_drops_total, 0U);
}

TEST_F(NewestOnly, ANewestOnlyTriggerStepsOnceOnTheNewestAndCountsTheRest)
{
    Mux        mux(true);
    const auto started = mux.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    EXPECT_EQ(this->backlogOf(mux, 1), (std::vector<double>{3.0}));
    const auto health = mux.cmd.bindingHealth();
    ASSERT_TRUE(health.has_value());
    EXPECT_EQ(health->work_limit_drops_total, 2U);
    EXPECT_EQ(health->overrun_total, 0U) << "nothing is lost in transport";
}
