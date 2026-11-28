// test-layer: unit (no node, no bus, no C function; #336)
// The checked conversion to the RosTime wire type. A split that wraps a negative or a
// too-large time writes a plausible but wrong stamp, so the conversion refuses those values.

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_msgs/Header.h>

#include <cstdint>
#include <limits>
#include <optional>
#include <stdexcept>
#include <vector>

namespace {

constexpr int64_t NANOS_PER_SECOND = 1'000'000'000;
constexpr int64_t MAX_WIRE_SECONDS = std::numeric_limits<uint32_t>::max();

TEST(MessageStamp, TheConversionKeepsTheNanosecondRemainderExactly)
{
    const hypha_msgs::RosTime stamp = hypha::toRosTime(hypha::Time::fromNanos(1'500'000'001));

    EXPECT_EQ(stamp.secs, 1U);
    EXPECT_EQ(stamp.nsecs, 500'000'001U);
}

TEST(MessageStamp, TheWireRangeConvertsFromItsFirstToItsLastNanosecond)
{
    const hypha_msgs::RosTime first = hypha::toRosTime(hypha::Time::fromNanos(0));
    const hypha_msgs::RosTime last = hypha::toRosTime(
        hypha::Time::fromNanos(MAX_WIRE_SECONDS * NANOS_PER_SECOND + NANOS_PER_SECOND - 1)
    );

    EXPECT_EQ(first.secs, 0U);
    EXPECT_EQ(first.nsecs, 0U);
    EXPECT_EQ(last.secs, std::numeric_limits<uint32_t>::max());
    EXPECT_EQ(last.nsecs, 999'999'999U);
}

TEST(MessageStamp, ANegativeTimeHasNoWireValue)
{
    EXPECT_THROW((void)hypha::toRosTime(hypha::Time::fromNanos(-1)), std::out_of_range);
}

TEST(MessageStamp, OneSecondPastTheWireRangeHasNoWireValue)
{
    const hypha::Time pastRange = hypha::Time::fromNanos((MAX_WIRE_SECONDS + 1) * NANOS_PER_SECOND);

    EXPECT_THROW((void)hypha::toRosTime(pastRange), std::out_of_range);
}

TEST(MessageStamp, ASerializedHeaderKeepsBothStampFields)
{
    const hypha_msgs::RosTime stamp =
        hypha::toRosTime(hypha::Time::fromNanos(1'700'000'000'123'456'789));
    std_msgs::Header header;
    header.seq = 3;
    header.stamp = stamp;
    header.frame_id = "map";

    const std::vector<uint8_t> bytes = hypha::codec::encode(header);
    std_msgs::Header           decoded;
    ASSERT_TRUE(hypha::codec::decode(bytes, decoded));

    EXPECT_EQ(decoded.stamp.secs, 1'700'000'000U);
    EXPECT_EQ(decoded.stamp.nsecs, 123'456'789U);
}

TEST(MessageStamp, FromRosTimeChecksTheRemainderAndReconstructsNanoseconds)
{
    const hypha_msgs::RosTime stamp{2U, 123'456'789U};
    const auto                converted = hypha::fromRosTime(stamp);
    ASSERT_TRUE(converted.has_value());
    EXPECT_EQ(converted->nanos(), 2'123'456'789LL);

    const hypha_msgs::RosTime malformed{2U, 1'000'000'000U};
    EXPECT_FALSE(hypha::fromRosTime(malformed).has_value());
}

}  // namespace

namespace {

/// Turns simulated time off when the test ends, also when an assertion fails.
class SimTimeOff
{
  public:
    ~SimTimeOff()
    {
        const bool disabled = hypha::disableSimTime();
        EXPECT_TRUE(disabled);
    }
};

}  // namespace

TEST(MessageStamp, ANowStampReadsSimulatedTimeWhileItIsActive)
{
    SimTimeOff restore;
    ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1'500'000'123)));
    const hypha::Time         current = hypha::now();
    const hypha_msgs::RosTime stamp = hypha::toRosTime(current);
    EXPECT_EQ(stamp.secs, 1U);
    EXPECT_EQ(stamp.nsecs, 500'000'123U);
}

TEST(MessageStamp, ANowStampReadsUtcTimeWhileSimulatedTimeIsOff)
{
    SimTimeOff restore;
    ASSERT_TRUE(hypha::disableSimTime());
    const hypha::Time before = hypha::now();
    const hypha::Time current = hypha::now();
    const hypha::Time after = hypha::now();
    ASSERT_FALSE(hypha::isSimTime());
    EXPECT_NO_THROW((void)hypha::toRosTime(current));
    EXPECT_GT(current.seconds(), 1.0e9);
    EXPECT_GE(current.nanos(), before.nanos());
    EXPECT_LE(current.nanos(), after.nanos());
}
