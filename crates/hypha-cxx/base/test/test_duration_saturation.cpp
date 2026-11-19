#include "hypha_base/time.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <limits>

using hypha::Duration;
using namespace hypha::literals;

namespace {

constexpr int64_t kMaxNanos = std::numeric_limits<int64_t>::max();
constexpr int64_t kMinNanos = std::numeric_limits<int64_t>::min();

}  // namespace

TEST(DurationSaturation, AdditionClampsAtTheLimits)
{
    const Duration maxSpan = Duration::fromNanos(kMaxNanos);
    const Duration minSpan = Duration::fromNanos(kMinNanos);
    EXPECT_EQ((maxSpan + Duration::fromNanos(1)).nanos(), kMaxNanos);
    EXPECT_EQ((maxSpan + maxSpan).nanos(), kMaxNanos);
    EXPECT_EQ((minSpan + Duration::fromNanos(-1)).nanos(), kMinNanos);
}

TEST(DurationSaturation, SubtractionClampsAtTheLimits)
{
    const Duration maxSpan = Duration::fromNanos(kMaxNanos);
    const Duration minSpan = Duration::fromNanos(kMinNanos);
    EXPECT_EQ((minSpan - Duration::fromNanos(1)).nanos(), kMinNanos);
    EXPECT_EQ((maxSpan - Duration::fromNanos(-1)).nanos(), kMaxNanos);
}

TEST(DurationSaturation, NegatingTheMinimumClampsToTheMaximum)
{
    EXPECT_EQ((-Duration::fromNanos(kMinNanos)).nanos(), kMaxNanos);
}

TEST(DurationSaturation, CompoundAssignmentClampsAtTheLimits)
{
    Duration up = Duration::fromNanos(kMaxNanos);
    up += Duration::fromNanos(1);
    EXPECT_EQ(up.nanos(), kMaxNanos);

    Duration down = Duration::fromNanos(kMinNanos);
    down -= Duration::fromNanos(1);
    EXPECT_EQ(down.nanos(), kMinNanos);
}

TEST(DurationSaturation, MultiplicationClampsAtTheLimits)
{
    const Duration maxSpan = Duration::fromNanos(kMaxNanos);
    const Duration minSpan = Duration::fromNanos(kMinNanos);
    EXPECT_EQ((maxSpan * 2.0).nanos(), kMaxNanos);
    EXPECT_EQ((minSpan * 2.0).nanos(), kMinNanos);
    EXPECT_EQ((maxSpan * -2.0).nanos(), kMinNanos);
    EXPECT_EQ((3.0 * maxSpan).nanos(), kMaxNanos);
    EXPECT_EQ((100_ms * 1e300).nanos(), kMaxNanos);
}

TEST(DurationSaturation, NonFiniteFactorGivesZero)
{
    const double nan = std::numeric_limits<double>::quiet_NaN();
    const double inf = std::numeric_limits<double>::infinity();
    EXPECT_TRUE((100_ms * nan).isZero());
    EXPECT_TRUE((100_ms * inf).isZero());
    EXPECT_TRUE((100_ms * -inf).isZero());
}

TEST(DurationSaturation, IntegerFactoriesClampAtTheLimits)
{
    EXPECT_EQ(Duration::fromMicros(std::numeric_limits<int64_t>::max()).nanos(), kMaxNanos);
    EXPECT_EQ(Duration::fromMillis(std::numeric_limits<int64_t>::min()).nanos(), kMinNanos);
    EXPECT_EQ(Duration::fromMillis(std::numeric_limits<int64_t>::max()).nanos(), kMaxNanos);
}

TEST(DurationSaturation, NonFiniteSecondsGiveZero)
{
    const double nan = std::numeric_limits<double>::quiet_NaN();
    const double inf = std::numeric_limits<double>::infinity();
    EXPECT_TRUE(Duration::fromSeconds(nan).isZero());
    EXPECT_TRUE(Duration::fromSeconds(inf).isZero());
    EXPECT_TRUE(Duration::fromSeconds(-inf).isZero());
}

TEST(DurationSaturation, OutOfRangeSecondsClampAtTheLimits)
{
    EXPECT_EQ(Duration::fromSeconds(1e300).nanos(), kMaxNanos);
    EXPECT_EQ(Duration::fromSeconds(-1e300).nanos(), kMinNanos);
    EXPECT_EQ(Duration::fromSeconds(9.3e9).nanos(), kMaxNanos);
    EXPECT_EQ(Duration::fromSeconds(-9.3e9).nanos(), kMinNanos);
}

TEST(DurationSaturation, InRangeSecondsAreNotClamped)
{
    EXPECT_EQ(Duration::fromSeconds(9.2e9).nanos(), 9200000000000000000LL);
    EXPECT_EQ(Duration::fromSeconds(-9.2e9).nanos(), -9200000000000000000LL);
}
