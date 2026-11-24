// The supervision limits of every input port come from its table: a port of a composed
// hypha::Cycle (L2) reads silence_ms and stagnation_ms from [ports.inputs.<name>] as a
// CycleNode (L3) port does. supervised and the range options mean the same on both.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_msgs/UInt32.h>

#include <chrono>
#include <cstdint>
#include <expected>
#include <filesystem>
#include <fstream>
#include <functional>
#include <optional>
#include <string>
#include <thread>

using UInt32 = std_msgs::UInt32;
using namespace hypha::literals;

namespace {

std::string topicFor(const std::string& name)
{
    return hypha_test::testTopic("/" + name);
}

/// Runs short slices of simulated time until @p done holds, or fails after a real-time bound.
bool runUntil(hypha::Node& node, const std::function<bool()>& done, std::chrono::seconds bound)
{
    const auto deadline = std::chrono::steady_clock::now() + bound;
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

/// Calls @p publish once per 10 ms slice until @p done holds. The predicate only observes.
bool publishUntil(
    hypha::Node&                 node,
    const std::function<void()>& publish,
    const std::function<bool()>& done,
    std::chrono::seconds         bound
)
{
    const auto deadline = std::chrono::steady_clock::now() + bound;
    while (!done()) {
        if (std::chrono::steady_clock::now() >= deadline) {
            return false;
        }
        publish();
        hypha_test::advance(10_ms);
        if (!node.runFor(1_ms).ok()) {
            return false;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    return true;
}

class L2SupervisionFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path =
            std::filesystem::temp_directory_path() / "hypha_cycle_supervision_from_table.toml";
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    void initWith(const std::string& parameters)
    {
        std::ofstream output(this->path);
        output << parameters;
        output.close();
        this->node.emplace("cycle_supervision_from_table", this->path.string());
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

struct NoopStep
{
    void operator()(const hypha::CycleTime&) const
    {
    }
};

/// Starts one composed cycle with one Latest port named @p name with @p options and, when
/// @p alive is set, the alive value of UInt32::data.
template <typename... Options>
std::expected<void, std::string>
startComposed(hypha::Node& node, const std::string& name, bool alive, Options... options)
{
    hypha::Cycle          cycle("l2_" + name);
    hypha::Latest<UInt32> input{cycle, name, options...};
    if (alive) {
        input.setAlive(&UInt32::data);
    }
    cycle.setRate(hypha::Hz(100.0));
    return cycle.start(node, NoopStep{});
}

TEST_F(L2SupervisionFixture, ASilenceKeyMakesTheComposedPortStale)
{
    const std::string topic = topicFor("l2_stale");
    this->initWith("[ports.inputs.l2_stale]\ntopic='" + topic + "'\nsilence_ms = 100\n");
    hypha::Node& nodeRef = *this->node;
    auto         publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());

    hypha::Cycle          cycle("l2_stale_cycle");
    hypha::Latest<UInt32> input{cycle, std::string("l2_stale")};
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(nodeRef, NoopStep{}).has_value());

    ASSERT_TRUE(publisher.publish(UInt32{1}));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&input]() {
            return input.received() > 0U;
        },
        std::chrono::seconds(10)
    ));
    ASSERT_TRUE(runUntil(
        nodeRef,
        [&input]() {
            return input.stale();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_TRUE((input.stale() || input.stagnant()));
}

TEST_F(L2SupervisionFixture, AStagnationKeyWithSetAliveMakesTheComposedPortStagnant)
{
    const std::string topic = topicFor("l2_stagnant");
    this->initWith(
        "[ports.inputs.l2_stagnant]\ntopic='" + topic
        + "'\nsilence_ms = 1000\nstagnation_ms = 100\n"
    );
    hypha::Node& nodeRef = *this->node;
    auto         publisher = hypha::Node().advertise<UInt32>(hypha::Topic{topic});
    ASSERT_TRUE(publisher.enabled());

    hypha::Cycle          cycle("l2_stagnant_cycle");
    hypha::Latest<UInt32> input{cycle, std::string("l2_stagnant")};
    input.setAlive(&UInt32::data);
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(nodeRef, NoopStep{}).has_value());

    ASSERT_TRUE(publishUntil(
        nodeRef,
        [&publisher]() {
            EXPECT_TRUE(publisher.publish(UInt32{5}));
        },
        [&input]() {
            return input.stagnant();
        },
        std::chrono::seconds(10)
    ));
    EXPECT_FALSE(input.stale());
}

TEST_F(L2SupervisionFixture, ALimitMaxAppliesToAComposedPort)
{
    this->initWith(
        "[ports.inputs.l2_range]\ntopic='" + topicFor("l2_range") + "'\nsilence_ms = 500\n"
    );
    hypha::Node& nodeRef = *this->node;
    const auto   started = startComposed(
        nodeRef, "l2_range", false, hypha::limitMax(hypha::Duration::fromMillis(250))
    );
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(started.error().find("outside its range"), std::string::npos) << started.error();
}

}  // namespace
