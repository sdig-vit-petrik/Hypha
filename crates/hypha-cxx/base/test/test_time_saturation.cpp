#include "hypha_base/time.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <limits>

using hypha::Duration;
using hypha::Time;
using namespace hypha::literals;

namespace {

constexpr int64_t kMaxNanos = std::numeric_limits<int64_t>::max();
constexpr int64_t kMinNanos = std::numeric_limits<int64_t>::min();

}  // namespace

TEST(TimeSaturation, FromSecondsGivesZeroForNonFiniteInput)
{
    const double nan = std::numeric_limits<double>::quiet_NaN();
    const double inf = std::numeric_limits<double>::infinity();
    EXPECT_TRUE(Time::fromSeconds(nan).isZero());
    EXPECT_TRUE(Time::fromSeconds(inf).isZero());
    EXPECT_TRUE(Time::fromSeconds(-inf).isZero());
}

TEST(TimeSaturation, FromSecondsClampsOutOfRangeInput)
{
    EXPECT_EQ(Time::fromSeconds(1e300).nanos(), kMaxNanos);
    EXPECT_EQ(Time::fromSeconds(-1e300).nanos(), kMinNanos);
    EXPECT_EQ(Time::fromSeconds(9.3e9).nanos(), kMaxNanos);
    EXPECT_EQ(Time::fromSeconds(-9.3e9).nanos(), kMinNanos);
    EXPECT_EQ(Time::fromSeconds(9.2e9).nanos(), 9200000000000000000LL);
}

TEST(TimeSaturation, OffsetByDurationClampsAtTheLimits)
{
    const Time maxTime = Time::fromNanos(kMaxNanos);
    const Time minTime = Time::fromNanos(kMinNanos);
    EXPECT_EQ((maxTime + Duration::fromNanos(1)).nanos(), kMaxNanos);
    EXPECT_EQ((maxTime + Duration::fromNanos(kMaxNanos)).nanos(), kMaxNanos);
    EXPECT_EQ((minTime + Duration::fromNanos(-1)).nanos(), kMinNanos);
    EXPECT_EQ((minTime - Duration::fromNanos(1)).nanos(), kMinNanos);
    EXPECT_EQ((maxTime - Duration::fromNanos(-1)).nanos(), kMaxNanos);
}

TEST(TimeSaturation, CompoundOffsetClampsAtTheLimits)
{
    Time moving = Time::fromNanos(kMaxNanos);
    moving += 1_ns;
    EXPECT_EQ(moving.nanos(), kMaxNanos);

    Time negative = Time::fromNanos(kMinNanos);
    negative += Duration::fromNanos(-1);
    EXPECT_EQ(negative.nanos(), kMinNanos);
}

TEST(TimeSaturation, DifferenceClampsAtTheLimits)
{
    const Time maxTime = Time::fromNanos(kMaxNanos);
    const Time minTime = Time::fromNanos(kMinNanos);
    EXPECT_EQ((maxTime - Time::fromNanos(-1)).nanos(), kMaxNanos);
    EXPECT_EQ((minTime - Time::fromNanos(1)).nanos(), kMinNanos);
}

TEST(TimeSaturation, InRangeArithmeticIsExact)
{
    EXPECT_EQ((Time::fromNanos(100) + 50_ns).nanos(), 150);
    EXPECT_EQ((Time::fromNanos(100) - 50_ns).nanos(), 50);
    EXPECT_EQ((Time::fromNanos(150) - Time::fromNanos(100)).nanos(), 50);
}
