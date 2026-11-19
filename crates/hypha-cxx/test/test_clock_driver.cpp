// A cycle that drives /clock for a simulator. Every case owns the process clock, so the
// node runs no other simulated-time work.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include <hypha_msgs/hypha_msgs/ReplayProgress.h>
#include <hypha_msgs/rosgraph_msgs/Clock.h>
#include <hypha_msgs/std_msgs/Float64.h>

#include "test_types.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

using namespace hypha::literals;

namespace {

constexpr int64_t kStart = 1'000'000'000;
constexpr int64_t kPeriod = 10'000'000;
/// The one participant every driver expects: a driver with none is refused.
constexpr const char* kPeer = "clock_driver_peer";

/// A driver cycle with no ports that records each step's deadline.
class Simulator final : public hypha::CycleNode
{
  public:
    explicit Simulator(std::optional<double> factor = std::nullopt)
    {
        this->setRate(hypha::Hz(100.0));
        this->setClockDriver(
            hypha::ClockDriverOptions{
                .start = hypha::Time::fromNanos(kStart),
                .realTimeFactor = factor,
                .participants = {kPeer},
            }
        );
    }

    std::vector<int64_t> deadlines;

    void step(const hypha::CycleTime& time) override
    {
        this->deadlines.push_back(time.now.nanos());
    }
};

struct ReceiveClock
{
    std::vector<int64_t>* seen;

    void operator()(const hypha::Subscriber<rosgraph_msgs::Clock>::ConstPtr& message) const
    {
        const auto& clock = message->clock;
        this->seen->push_back(static_cast<int64_t>(clock.secs) * 1'000'000'000 + clock.nsecs);
    }
};

/// The participant: it answers each `/clock` sample on `/clock_progress`, as a node's
/// runtime does once it has run the sample's deadlines.
struct Peer
{
    hypha::Publisher<hypha_msgs::ReplayProgress> progress =
        hypha::Node().advertise<hypha_msgs::ReplayProgress>(hypha::Topic{"/clock_progress"});
    std::optional<hypha::RawSubscriber> clock;

    Peer()
    {
        auto subscribed = hypha::subscribeRawOfType(
            "/clock", "rosgraph_msgs/Clock", [this](const hypha::RawSample& sample) {
                uint32_t secs = 0;
                uint32_t nsecs = 0;
                if (sample.bytes.size() != 8) {
                    return;
                }
                std::memcpy(&secs, sample.bytes.data(), 4);
                std::memcpy(&nsecs, sample.bytes.data() + 4, 4);
                hypha_msgs::ReplayProgress report;
                report.node = kPeer;
                report.clock_nanos = static_cast<uint64_t>(secs) * 1'000'000'000U + nsecs;
                report.clock_sent_nanos = static_cast<uint64_t>(sample.transportSent.nanos());
                static_cast<void>(this->progress.publish(report));
            }
        );
        if (subscribed.has_value()) {
            this->clock.emplace(std::move(*subscribed));
        }
    }
};

class ClockDriverFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        static_cast<void>(hypha::disableSimTime());
        this->node.emplace("clock_driver_test");
    }

    void TearDown() override
    {
        this->node.reset();
        static_cast<void>(hypha::disableSimTime());
    }

    std::optional<hypha::Node> node;
};

}  // namespace

TEST_F(ClockDriverFixture, StepsAtStartPlusMultiplesOfThePeriod)
{
    Peer peer;
    ASSERT_TRUE(peer.clock.has_value());
    Simulator  simulator;
    const auto started = simulator.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    ASSERT_TRUE(this->node->runFor(50_ms).ok());
    ASSERT_GE(simulator.deadlines.size(), 3U);
    for (size_t index = 0; index < simulator.deadlines.size(); ++index) {
        EXPECT_EQ(simulator.deadlines[index], kStart + static_cast<int64_t>(index) * kPeriod);
    }
}

TEST_F(ClockDriverFixture, PublishesClockAfterEachStep)
{
    Peer peer;
    ASSERT_TRUE(peer.clock.has_value());
    Simulator            simulator;
    std::vector<int64_t> seen;
    auto                 clock =
        hypha::Node().subscribe<rosgraph_msgs::Clock>(hypha::Topic{"/clock"}, ReceiveClock{&seen});
    const auto started = simulator.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    ASSERT_TRUE(this->node->runFor(50_ms).ok());
    ASSERT_FALSE(seen.empty());
    const std::vector<int64_t> expected(
        simulator.deadlines.begin(), simulator.deadlines.begin() + static_cast<long>(seen.size())
    );
    EXPECT_EQ(seen, expected);
}

TEST_F(ClockDriverFixture, PacesToTheRealTimeFactor)
{
    Peer peer;
    ASSERT_TRUE(peer.clock.has_value());
    Simulator  simulator(1.0);
    const auto started = simulator.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    ASSERT_TRUE(this->node->runFor(200_ms).ok());
    EXPECT_GE(simulator.deadlines.size(), 18U);
    EXPECT_LE(simulator.deadlines.size(), 23U);
}

class Alone final : public hypha::CycleNode
{
  public:
    Alone()
    {
        this->setRate(hypha::Hz(100.0));
        this->setClockDriver({});
    }

    void step(const hypha::CycleTime&) override
    {
    }
};

TEST_F(ClockDriverFixture, WithoutAParticipantIsRefused)
{
    Alone      simulator;
    const auto started = simulator.start(*this->node);
    ASSERT_FALSE(started.has_value()) << "it would step before any node joins";
}

class NoRate final : public hypha::CycleNode
{
  public:
    NoRate()
    {
        this->setClockDriver({});
    }

    void step(const hypha::CycleTime&) override
    {
    }
};

TEST_F(ClockDriverFixture, WithoutARateIsRefused)
{
    NoRate     simulator;
    const auto started = simulator.start(*this->node);
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(started.error().find("cycle 'clock_driver_test'"), std::string::npos)
        << started.error();
}

class TriggeredDriver final : public hypha::CycleNode
{
  public:
    TriggeredDriver()
    {
        this->setTrigger(this->input);
        this->setClockDriver({});
    }

    hypha::Arrivals<std_msgs::Float64> input{*this, "driver_in"};

    void step(const hypha::CycleTime&) override
    {
    }
};

TEST_F(ClockDriverFixture, WithATriggerIsRefused)
{
    TriggeredDriver simulator;
    const auto      started = simulator.start(*this->node);
    ASSERT_FALSE(started.has_value());
    EXPECT_NE(started.error().find("cycle 'clock_driver_test'"), std::string::npos)
        << started.error();
}
