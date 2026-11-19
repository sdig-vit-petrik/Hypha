#include "hypha_base/clock.h"

#include <gtest/gtest.h>

#include <thread>

using hypha::Duration;
using hypha::Time;
using namespace hypha::literals;

namespace {

class ClockFixture : public ::testing::Test
{
  protected:
    void TearDown() override
    {
        EXPECT_TRUE(hypha::disableSimTime());
    }
};

}  // namespace

TEST_F(ClockFixture, FallsBackToMonotonicWithNoSource)
{
    ASSERT_TRUE(hypha::disableSimTime());
    EXPECT_FALSE(hypha::isSimTime());

    const Time first = hypha::now();
    std::this_thread::sleep_for(std::chrono::milliseconds(5));
    EXPECT_GT(hypha::now(), first);
}

TEST_F(ClockFixture, EnabledSimTimeDrivesNow)
{
    ASSERT_TRUE(hypha::enableSimTime(Time::fromSeconds(100.0)));

    EXPECT_TRUE(hypha::isSimTime());
    EXPECT_DOUBLE_EQ(hypha::now().seconds(), 100.0);

    ASSERT_TRUE(hypha::advanceSimTime(2_s));
    EXPECT_DOUBLE_EQ(hypha::now().seconds(), 102.0);
}

TEST_F(ClockFixture, SetSimTimeMovesNow)
{
    ASSERT_TRUE(hypha::enableSimTime(Time::fromSeconds(100.0)));

    ASSERT_TRUE(hypha::setSimTime(Time::fromSeconds(7.5)));

    EXPECT_DOUBLE_EQ(hypha::now().seconds(), 7.5);
}

TEST_F(ClockFixture, SimulatedTimeDoesNotAdvanceOnItsOwn)
{
    ASSERT_TRUE(hypha::enableSimTime(Time::fromSeconds(50.0)));

    const Time before = hypha::now();
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
    EXPECT_EQ(hypha::now(), before);
}

TEST_F(ClockFixture, SimulatedTimeStartsAtZeroMeaningUnknown)
{
    ASSERT_TRUE(hypha::enableSimTime(Time::fromNanos(0)));
    EXPECT_TRUE(hypha::now().isZero());
}

TEST_F(ClockFixture, SetAndAdvanceAreRefusedWhileSimTimeIsDisabled)
{
    ASSERT_TRUE(hypha::disableSimTime());

    EXPECT_FALSE(hypha::setSimTime(Time::fromSeconds(1.0)));
    EXPECT_FALSE(hypha::advanceSimTime(1_s));
    EXPECT_FALSE(hypha::isSimTime());
}

TEST_F(ClockFixture, EnablingRefusesANegativeReading)
{
    EXPECT_FALSE(hypha::enableSimTime(Time::fromNanos(-1)));
    EXPECT_FALSE(hypha::isSimTime());
}

TEST_F(ClockFixture, SetRefusesANegativeReadingAndKeepsTheClock)
{
    ASSERT_TRUE(hypha::enableSimTime(Time::fromSeconds(10.0)));

    EXPECT_FALSE(hypha::setSimTime(Time::fromNanos(-1)));
    EXPECT_DOUBLE_EQ(hypha::now().seconds(), 10.0);
}

TEST_F(ClockFixture, AdvanceRefusesANegativeSpanAndKeepsTheClock)
{
    ASSERT_TRUE(hypha::enableSimTime(Time::fromSeconds(10.0)));

    EXPECT_FALSE(hypha::advanceSimTime(Duration::fromNanos(-1)));
    EXPECT_DOUBLE_EQ(hypha::now().seconds(), 10.0);
}

TEST_F(ClockFixture, LiveNowReadsUtcEpochTime)
{
    ASSERT_TRUE(hypha::disableSimTime());

    EXPECT_GT(hypha::now().seconds(), 1.0e9);
}

TEST_F(ClockFixture, DisablingRestoresTheFallback)
{
    ASSERT_TRUE(hypha::enableSimTime(Time::fromNanos(0)));
    ASSERT_TRUE(hypha::isSimTime());

    ASSERT_TRUE(hypha::disableSimTime());
    EXPECT_FALSE(hypha::isSimTime());
}

TEST_F(ClockFixture, SteadyClockAdvancesIndependentlyOfSimulation)
{
    ASSERT_TRUE(hypha::enableSimTime(Time::fromSeconds(100.0)));

    const Time started = hypha::steadyNow();
    std::this_thread::sleep_for(std::chrono::milliseconds(5));
    EXPECT_GT(hypha::steadyNow(), started);
    EXPECT_TRUE(hypha::isSimTime());
}
