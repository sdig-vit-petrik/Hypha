// test-layer: unit (no node, no bus, no C function; #336)
#include "hypha_base/time.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <limits>
#include <optional>
#include <stdexcept>

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

TEST(DurationSaturation, NonFiniteFactorIsRefused)
{
    const double nan = std::numeric_limits<double>::quiet_NaN();
    const double inf = std::numeric_limits<double>::infinity();
    EXPECT_THROW(static_cast<void>(100_ms * nan), std::invalid_argument);
    EXPECT_THROW(static_cast<void>(100_ms * inf), std::invalid_argument);
    EXPECT_THROW(static_cast<void>(100_ms * -inf), std::invalid_argument);
    EXPECT_THROW(static_cast<void>(nan * 100_ms), std::invalid_argument);
}

TEST(DurationSaturation, IntegerFactoriesClampAtTheLimits)
{
    EXPECT_EQ(Duration::fromMicros(std::numeric_limits<int64_t>::max()).nanos(), kMaxNanos);
    EXPECT_EQ(Duration::fromMillis(std::numeric_limits<int64_t>::min()).nanos(), kMinNanos);
    EXPECT_EQ(Duration::fromMillis(std::numeric_limits<int64_t>::max()).nanos(), kMaxNanos);
}

TEST(DurationSaturation, NonFiniteSecondsAreRefused)
{
    const double nan = std::numeric_limits<double>::quiet_NaN();
    const double inf = std::numeric_limits<double>::infinity();
    EXPECT_THROW(static_cast<void>(Duration::fromSeconds(nan)), std::invalid_argument);
    EXPECT_THROW(static_cast<void>(Duration::fromSeconds(inf)), std::invalid_argument);
    EXPECT_THROW(static_cast<void>(Duration::fromSeconds(-inf)), std::invalid_argument);
}

TEST(DurationSaturation, TryFromSecondsGivesNothingForNonFiniteSeconds)
{
    const double nan = std::numeric_limits<double>::quiet_NaN();
    const double inf = std::numeric_limits<double>::infinity();
    EXPECT_FALSE(Duration::tryFromSeconds(nan).has_value());
    EXPECT_FALSE(Duration::tryFromSeconds(inf).has_value());
    EXPECT_FALSE(Duration::tryFromSeconds(-inf).has_value());
}

TEST(DurationSaturation, TryFromSecondsConvertsAndClampsFiniteSeconds)
{
    const std::optional<Duration> span = Duration::tryFromSeconds(1.5);
    ASSERT_TRUE(span.has_value());
    EXPECT_EQ(span->nanos(), 1500000000);
    const std::optional<Duration> clamped = Duration::tryFromSeconds(1e300);
    ASSERT_TRUE(clamped.has_value());
    EXPECT_EQ(clamped->nanos(), kMaxNanos);
}

TEST(DurationSaturation, FiniteSecondsStayUsableInConstantExpressions)
{
    constexpr Duration fiveSeconds = Duration::fromSeconds(5.0);
    static_assert(fiveSeconds.nanos() == 5000000000LL);
    constexpr Duration halved = fiveSeconds * 0.5;
    static_assert(halved.nanos() == 2500000000LL);
    constexpr std::optional<Duration> tried = Duration::tryFromSeconds(2.0);
    static_assert(tried.has_value() && tried->nanos() == 2000000000LL);
    EXPECT_EQ(fiveSeconds.nanos(), 5000000000LL);
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
