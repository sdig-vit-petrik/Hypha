#include "hypha_base/time.h"

#include <gtest/gtest.h>

using hypha::Duration;
using hypha::Time;
using namespace hypha::literals;

TEST(Duration, ConstructionUnitsAgree)
{
    EXPECT_EQ(Duration::fromNanos(1500000000).nanos(), 1500000000);
    EXPECT_EQ(Duration::fromMicros(1500000).nanos(), 1500000000);
    EXPECT_EQ(Duration::fromMillis(1500).nanos(), 1500000000);
    EXPECT_EQ(Duration::fromSeconds(1.5).nanos(), 1500000000);
    EXPECT_DOUBLE_EQ(Duration::fromMillis(1500).seconds(), 1.5);
}

TEST(Duration, Literals)
{
    EXPECT_EQ((100_ms).nanos(), 100000000);
    EXPECT_EQ((2_s).nanos(), 2000000000);
    EXPECT_EQ((500_us).nanos(), 500000);
    EXPECT_EQ((250_ns).nanos(), 250);
    EXPECT_EQ((1.5_s).nanos(), 1500000000);
}

TEST(Duration, Arithmetic)
{
    EXPECT_EQ((100_ms + 400_ms).nanos(), (500_ms).nanos());
    EXPECT_EQ((1_s - 250_ms).nanos(), (750_ms).nanos());
    EXPECT_EQ((-(100_ms)).nanos(), -100000000);
    EXPECT_EQ((100_ms * 2.5).nanos(), (250_ms).nanos());
    EXPECT_EQ((2.0 * 100_ms).nanos(), (200_ms).nanos());

    Duration d = 1_s;
    d += 500_ms;
    EXPECT_EQ(d.nanos(), (1500_ms).nanos());
    d -= 1_s;
    EXPECT_EQ(d.nanos(), (500_ms).nanos());
}

TEST(Duration, NegativeDurationsAreRepresentable)
{
    const Duration negative = 100_ms - 400_ms;
    EXPECT_LT(negative.nanos(), 0);
    EXPECT_DOUBLE_EQ(negative.seconds(), -0.3);
}

TEST(Duration, NegatingTheMostNegativeValueSaturates)
{
    const Duration negated = -Duration::fromNanos(INT64_MIN);
    EXPECT_EQ(negated.nanos(), INT64_MAX);
}

TEST(Duration, Ordering)
{
    EXPECT_TRUE(100_ms < 200_ms);
    EXPECT_TRUE(200_ms > 100_ms);
    EXPECT_TRUE(100_ms == 100_ms);
    EXPECT_TRUE(100_ms <= 100_ms);
    EXPECT_TRUE((-(100_ms)) < Duration());
}

TEST(Time, DefaultIsZeroAndZeroMeansUnknown)
{
    const Time defaulted;
    EXPECT_TRUE(defaulted.isZero());
    EXPECT_EQ(defaulted.nanos(), 0);
    EXPECT_EQ(defaulted, Time::fromNanos(0));
}

TEST(Time, DifferenceYieldsDuration)
{
    const Time a = Time::fromSeconds(10.0);
    const Time b = Time::fromSeconds(12.5);
    EXPECT_DOUBLE_EQ((b - a).seconds(), 2.5);
    EXPECT_DOUBLE_EQ((a - b).seconds(), -2.5);
}

TEST(Time, DurationOffsets)
{
    const Time base = Time::fromSeconds(100.0);
    EXPECT_DOUBLE_EQ((base + 500_ms).seconds(), 100.5);
    EXPECT_DOUBLE_EQ((base - 500_ms).seconds(), 99.5);

    Time moving = base;
    moving += 1_s;
    EXPECT_DOUBLE_EQ(moving.seconds(), 101.0);
}

TEST(Time, Ordering)
{
    EXPECT_TRUE(Time::fromSeconds(1.0) < Time::fromSeconds(2.0));
    EXPECT_TRUE(Time::fromSeconds(2.0) >= Time::fromSeconds(2.0));
    EXPECT_TRUE(Time::fromSeconds(2.0) == Time::fromSeconds(2.0));
}

TEST(Constexpr, UsableAtCompileTime)
{
    constexpr Duration period = 100_ms;
    constexpr Time     start = Time::fromNanos(0);
    constexpr Time     end = start + period;
    static_assert(end.nanos() == 100000000);
    static_assert((end - start).nanos() == period.nanos());
    SUCCEED();
}
