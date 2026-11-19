// Level 2, the step without a bus: hypha::testing::StepHarness runs ScanCycle::step() on
// inputs the test sets. One case for each verdict of each input, and for recovery.
// deliver() hands a port a value with its send time; each step judges it against
// `silence_ms` of test/scan_cycle_test.toml.

#include <hypha/hypha.h>

HYPHA_LOG_MODULE_INIT("scan_cycle")

#include <hypha/testing.h>

#include "scan_cycle/scan_cycle.h"

#include <gtest/gtest.h>

#include <cmath>
#include <cstdint>
#include <limits>
#include <memory>
#include <vector>

namespace {

using hypha::InputVerdict;
using hypha::Time;
using scan_cycle::ScanCycle;
using Float64 = ScanCycle::Float64;
using Command = ScanCycle::Command;

constexpr int64_t kMilli = 1'000'000;
constexpr int64_t kStart = 50'000 * kMilli;  // 50 s
constexpr int64_t kSpeedLimit = 200 * kMilli;  // silence_ms of the speed input
constexpr int64_t kSetpointLimit = 500 * kMilli;

/// The CycleTime of a step at @p nanos: `now`, and `stamp` as the cycle derives it.
hypha::CycleTime stepAt(int64_t nanos)
{
    hypha::CycleTime time;
    time.now = Time::fromNanos(nanos);
    time.stamp = hypha::toRosTime(time.now);
    return time;
}

/// The parameter file, a cycle that was not started, and its harness.
class ScanCycleStep : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->configuration = std::make_unique<hypha::ConfigContext>(SCAN_CYCLE_TEST_CONFIG);
        this->node = std::make_unique<ScanCycle>();
        this->harness = std::make_unique<hypha::testing::StepHarness>(*this->node);
    }

    void TearDown() override
    {
        this->harness.reset();
        this->node.reset();
        this->configuration.reset();
    }

    /// Runs one step at @p nanos and returns its one command.
    Command stepOnce(int64_t nanos)
    {
        this->harness->step(*this->node, stepAt(nanos));
        const std::vector<Command>& sent = this->harness->sent(this->node->command);
        EXPECT_EQ(sent.size(), 1U);
        return sent.empty() ? Command{} : sent.front();
    }

    std::unique_ptr<hypha::ConfigContext>        configuration;
    std::unique_ptr<ScanCycle>                   node;
    std::unique_ptr<hypha::testing::StepHarness> harness;
};

}  // namespace

TEST_F(ScanCycleStep, ASpeedThatNeverArrivedCommandsNoMotion)
{
    // Stated, not assumed: a port the test never set answers as before start(), and its
    // health is not judged.
    this->harness->setNeverArrived(this->node->speed);
    const Command command = this->stepOnce(kStart);
    EXPECT_EQ(this->node->speed.verdict(), InputVerdict::NeverArrived);
    EXPECT_DOUBLE_EQ(command.twist.linear.x, 0.0);
    EXPECT_FALSE(this->node->inputsOk());
    EXPECT_EQ(this->node->noSpeedSteps(), 1U);
}

TEST_F(ScanCycleStep, AUsableSpeedIsControlledTowardsTheFallbackSetpoint)
{
    this->harness->deliver(this->node->speed, Float64{4.0}, Time::fromNanos(kStart));
    const Command command = this->stepOnce(kStart + 10 * kMilli);
    EXPECT_EQ(this->node->speed.verdict(), InputVerdict::Usable);
    EXPECT_DOUBLE_EQ(command.twist.linear.x, 3.0);  // (10 - 4) * 0.5
    EXPECT_TRUE(this->node->inputsOk());
    EXPECT_EQ(this->node->noSpeedSteps(), 0U);
}

TEST_F(ScanCycleStep, TheCommandCarriesTheStampOfItsStepAndTheConfiguredFrame)
{
    this->harness->deliver(this->node->speed, Float64{4.0}, Time::fromNanos(kStart));
    const int64_t first = kStart + 10 * kMilli;
    const Command command = this->stepOnce(first);
    EXPECT_EQ(command.header.stamp.secs, 50U);
    EXPECT_EQ(command.header.stamp.nsecs, 10U * kMilli);
    EXPECT_EQ(command.header.frame_id, "base_link");
    const Command next = this->stepOnce(first + 10 * kMilli);
    EXPECT_EQ(next.header.stamp.nsecs, 20U * kMilli);
}

TEST_F(ScanCycleStep, ASilentSpeedCommandsNoMotionOnceItsLimitHasPassed)
{
    this->harness->deliver(this->node->speed, Float64{4.0}, Time::fromNanos(kStart));

    // At the limit, inclusive: still usable.
    const Command atLimit = this->stepOnce(kStart + kSpeedLimit);
    EXPECT_DOUBLE_EQ(atLimit.twist.linear.x, 3.0);
    EXPECT_TRUE(this->node->inputsOk());

    const Command pastLimit = this->stepOnce(kStart + kSpeedLimit + 1);
    EXPECT_EQ(this->node->speed.verdict(), InputVerdict::Stale);
    EXPECT_TRUE(this->node->speed.stale());
    EXPECT_DOUBLE_EQ(pastLimit.twist.linear.x, 0.0);
    EXPECT_FALSE(this->node->inputsOk());
    // The value is still held; the step must not use it.
    EXPECT_TRUE(this->node->speed.has());
    EXPECT_DOUBLE_EQ(this->node->speed.lastKnown().data, 4.0);
}

TEST_F(ScanCycleStep, ANewSpeedAfterSilenceRecoversInTheSameStep)
{
    this->harness->deliver(this->node->speed, Float64{4.0}, Time::fromNanos(kStart));
    const int64_t silent = kStart + 5 * kSpeedLimit;
    EXPECT_DOUBLE_EQ(this->stepOnce(silent).twist.linear.x, 0.0);

    this->harness->deliver(this->node->speed, Float64{6.0}, Time::fromNanos(silent + kMilli));
    const Command recovered = this->stepOnce(silent + 10 * kMilli);
    EXPECT_EQ(this->node->speed.verdict(), InputVerdict::Usable);
    EXPECT_DOUBLE_EQ(recovered.twist.linear.x, 2.0);  // (10 - 6) * 0.5
    EXPECT_TRUE(this->node->inputsOk());
}

TEST_F(ScanCycleStep, ASpeedThatIsNotFiniteCommandsNoMotion)
{
    const double nan = std::numeric_limits<double>::quiet_NaN();
    this->harness->deliver(this->node->speed, Float64{nan}, Time::fromNanos(kStart));
    const Command command = this->stepOnce(kStart + kMilli);
    // The verdict is about age: the value is usable, and the step refuses its content.
    EXPECT_EQ(this->node->speed.verdict(), InputVerdict::Usable);
    EXPECT_DOUBLE_EQ(command.twist.linear.x, 0.0);
    EXPECT_EQ(this->node->noSpeedSteps(), 1U);
}

TEST_F(ScanCycleStep, AUsableSetpointReplacesTheFallback)
{
    this->harness->deliver(this->node->speed, Float64{4.0}, Time::fromNanos(kStart));
    this->harness->deliver(this->node->setpoint, Float64{12.0}, Time::fromNanos(kStart));
    const Command command = this->stepOnce(kStart + kMilli);
    EXPECT_DOUBLE_EQ(command.twist.linear.x, 4.0);  // (12 - 4) * 0.5
}

TEST_F(ScanCycleStep, ASilentSetpointFallsBackAndDoesNotDegradeTheHealth)
{
    this->harness->deliver(this->node->setpoint, Float64{12.0}, Time::fromNanos(kStart));
    const int64_t later = kStart + kSetpointLimit + kMilli;
    this->harness->deliver(this->node->speed, Float64{4.0}, Time::fromNanos(later));
    const Command command = this->stepOnce(later);
    EXPECT_EQ(this->node->setpoint.verdict(), InputVerdict::Stale);
    EXPECT_DOUBLE_EQ(command.twist.linear.x, 3.0);  // the fallback, 10
    // advisory: a stale setpoint is not a fault of the node.
    EXPECT_TRUE(this->node->inputsOk());
}

TEST_F(ScanCycleStep, ASetpointThatNeverArrivedFallsBack)
{
    this->harness->deliver(this->node->speed, Float64{4.0}, Time::fromNanos(kStart));
    const Command command = this->stepOnce(kStart + kMilli);
    EXPECT_EQ(this->node->setpoint.verdict(), InputVerdict::NeverArrived);
    EXPECT_DOUBLE_EQ(command.twist.linear.x, 3.0);
    EXPECT_TRUE(this->node->inputsOk());
}
