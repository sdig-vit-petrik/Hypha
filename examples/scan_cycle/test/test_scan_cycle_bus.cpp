// Level 3, the bus: the started cycle with real transport, on simulated time so the test
// is deterministic. The test is the speed publisher and the command subscriber.
//
// Timeline (T0 = 100 s, period 10 ms, speed silence_ms = 200):
//   speed 4.0 sent at T0            -> command 3.0 for the deadlines T0+10 .. T0+200 ms
//   nothing more                    -> command 0.0 from T0+210 ms (the speed is stale)
//   speed 6.0 sent at T0+400 ms     -> command 2.0 from T0+410 ms (recovered)

#include <hypha/hypha.h>

HYPHA_LOG_MODULE_INIT("scan_cycle")

#include "scan_cycle/scan_cycle.h"

#include <gtest/gtest.h>
#include <unistd.h>

#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <string>
#include <vector>

namespace {

using hypha::Time;
using scan_cycle::ScanCycle;
using Float64 = ScanCycle::Float64;
using Command = ScanCycle::Command;

constexpr int64_t kMilli = 1'000'000;
constexpr int64_t kStart = 100'000 * kMilli;  // T0
constexpr int64_t kPeriod = 10 * kMilli;

struct Received
{
    int64_t stampNanos;
    double  value;
};

/// The subscriber callback: a named functor, as it outlives the call that registers it.
struct CollectCommands
{
    std::vector<Received>* received;

    void operator()(const hypha::MessagePtr<Command>& message) const
    {
        const int64_t secs = static_cast<int64_t>(message->header.stamp.secs);
        const int64_t nsecs = static_cast<int64_t>(message->header.stamp.nsecs);
        this->received->push_back(Received{secs * 1'000 * kMilli + nsecs, message->twist.linear.x});
    }
};

/// Runs @p node until a command stamped @p stampNanos or later has arrived; false after
/// 20 s of real time or when a run fails.
bool runUntilStamp(hypha::Node& node, const std::vector<Received>& received, int64_t stampNanos)
{
    const auto giveUp = std::chrono::steady_clock::now() + std::chrono::seconds(20);
    while (received.empty() || received.back().stampNanos < stampNanos) {
        if (std::chrono::steady_clock::now() >= giveUp) {
            return false;
        }
        if (!node.runFor(hypha::Duration::fromMillis(1)).ok()) {
            return false;
        }
    }
    return true;
}

/// Moves simulated time to @p targetNanos in steps of 50 ms and runs the node through
/// each, so the subscriber reads the commands of one slice before the next is published.
bool advanceTo(
    hypha::Node& node, const std::vector<Received>& received, int64_t fromNanos, int64_t targetNanos
)
{
    constexpr int64_t slice = 50 * kMilli;
    for (int64_t now = fromNanos + slice; now <= targetNanos; now += slice) {
        if (!hypha::setSimTime(Time::fromNanos(now))) {
            return false;
        }
        if (!runUntilStamp(node, received, now)) {
            return false;
        }
    }
    return true;
}

}  // namespace

TEST(ScanCycleBus, TheCommandFollowsTheSpeedThroughSilenceAndRecovery)
{
    // A domain of its own under the test root: the test never meets a system that runs
    // on this host, and `clean-shm.sh --domain t_` removes it once the test ended.
    const std::string domain = "t_" + std::to_string(::getpid()) + "_scan";
    ASSERT_EQ(::setenv("HYPHA_DOMAIN", domain.c_str(), 1), 0);
    ASSERT_TRUE(hypha::enableSimTime(Time::fromNanos(kStart)));
    std::vector<Received> received;
    {
        hypha::Node                            node("scan_cycle_bus_test", SCAN_CYCLE_TEST_CONFIG);
        ScanCycle                              cycle;
        const std::expected<void, std::string> started = cycle.start(node);
        ASSERT_TRUE(started.has_value()) << started.error();
        hypha::Publisher<Float64> speed =
            hypha::Node().advertise<Float64>(hypha::Topic{"/scan_cycle_test/speed"});
        hypha::Subscriber<Command> commands = hypha::Node().subscribe<Command>(
            hypha::Topic{"/scan_cycle_bus_test/command"}, CollectCommands{&received}
        );

        ASSERT_TRUE(speed.publish(Float64{4.0}));
        ASSERT_TRUE(node.runFor(hypha::Duration::fromMillis(1)).ok());
        ASSERT_TRUE(advanceTo(node, received, kStart, kStart + 200 * kMilli));
        EXPECT_TRUE(cycle.inputsOk());

        ASSERT_TRUE(advanceTo(node, received, kStart + 200 * kMilli, kStart + 400 * kMilli));
        EXPECT_FALSE(cycle.inputsOk());
        EXPECT_TRUE(cycle.speed.stale());

        ASSERT_TRUE(speed.publish(Float64{6.0}));
        ASSERT_TRUE(node.runFor(hypha::Duration::fromMillis(1)).ok());
        ASSERT_TRUE(advanceTo(node, received, kStart + 400 * kMilli, kStart + 500 * kMilli));
        EXPECT_TRUE(cycle.inputsOk());

        // No command was lost on the way to the subscriber.
        EXPECT_EQ(commands.missingSequences(), 0U);
    }
    ASSERT_TRUE(hypha::disableSimTime());

    // One command for each deadline after T0, in order, stamped with its deadline.
    std::vector<Received> afterStart;
    for (const Received& item : received) {
        if (item.stampNanos > kStart) {
            afterStart.push_back(item);
        }
    }
    ASSERT_EQ(afterStart.size(), 50U);
    for (size_t index = 0; index < afterStart.size(); ++index) {
        const int64_t offset = static_cast<int64_t>(index + 1) * kPeriod;
        EXPECT_EQ(afterStart[index].stampNanos, kStart + offset) << "command " << index;
        // A sample sent at T belongs to the deadlines after T; it is usable while its age
        // is at most the limit, inclusive.
        const bool firstSpeed = offset <= 200 * kMilli;
        const bool secondSpeed = offset > 400 * kMilli;
        double     wanted = 0.0;
        if (firstSpeed) {
            wanted = 3.0;  // (10 - 4) * 0.5
        }
        if (secondSpeed) {
            wanted = 2.0;  // (10 - 6) * 0.5
        }
        EXPECT_DOUBLE_EQ(afterStart[index].value, wanted) << "command " << index;
    }
}
