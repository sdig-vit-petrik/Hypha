// A throttled line writes on the first call, then at most once per period.
// An invalid period suppresses the line and counts a rejected line.

#include <hypha/metrics.h>
#include <hypha_log/hypha_log.h>

#include <gtest/gtest.h>

#include <chrono>
#include <cmath>
#include <limits>
#include <optional>
#include <thread>

HYPHA_LOG_MODULE_INIT("test/log_throttle")

namespace {

struct LogCounters
{
    uint64_t linesWritten{0};
    uint64_t rejectedLines{0};
};

LogCounters readHealth()
{
    const std::optional<uint64_t> lines = hypha::metric("log.lines_written");
    const std::optional<uint64_t> rejected = hypha::metric("log.rejected_lines");
    EXPECT_TRUE(lines.has_value() && rejected.has_value());
    return LogCounters{lines.value_or(0), rejected.value_or(0)};
}

void throttledOnce(double periodSeconds)
{
    HYPHA_LOG_ERROR_THROTTLE(periodSeconds, "throttled line");
}

}  // namespace

TEST(LogThrottle, firstCallWritesAndTheSecondCallInsidePeriodDoesNot)
{
    const uint64_t before = readHealth().linesWritten;
    for (int call = 0; call < 5; ++call) {
        throttledOnce(3600.0);
    }
    const uint64_t after = readHealth().linesWritten;
    EXPECT_EQ(after - before, 1U);
}

TEST(LogThrottle, aCallAfterThePeriodWritesAgain)
{
    const uint64_t before = readHealth().linesWritten;
    const double   periodSeconds = 0.05;
    for (int call = 0; call < 3; ++call) {
        HYPHA_LOG_ERROR_THROTTLE(periodSeconds, "slow loop {}", call);
        std::this_thread::sleep_for(std::chrono::milliseconds(80));
    }
    const uint64_t after = readHealth().linesWritten;
    EXPECT_EQ(after - before, 3U);
}

TEST(LogThrottle, eachCallSiteHasItsOwnThrottle)
{
    const uint64_t before = readHealth().linesWritten;
    HYPHA_LOG_ERROR_THROTTLE(3600.0, "site one");
    HYPHA_LOG_ERROR_THROTTLE(3600.0, "site two");
    const uint64_t after = readHealth().linesWritten;
    EXPECT_EQ(after - before, 2U);
}

TEST(LogThrottle, anInvalidPeriodSuppressesTheLineAndCountsARejectedLine)
{
    const double invalidPeriods[] = {
        0.0, -1.0, std::numeric_limits<double>::quiet_NaN(), std::numeric_limits<double>::infinity()
    };
    for (const double period : invalidPeriods) {
        const LogCounters before = readHealth();
        throttledOnce(period);
        const LogCounters after = readHealth();
        EXPECT_EQ(after.linesWritten, before.linesWritten) << "period " << period;
        EXPECT_EQ(after.rejectedLines - before.rejectedLines, 1U) << "period " << period;
    }
}

TEST(LogThrottle, anInvalidPeriodDoesNotRemoveTheThrottle)
{
    const uint64_t before = readHealth().linesWritten;
    const double   periods[] = {0.0, 3600.0, 3600.0};
    for (const double period : periods) {
        HYPHA_LOG_ERROR_THROTTLE(period, "same call site");
    }
    const uint64_t after = readHealth().linesWritten;
    EXPECT_EQ(after - before, 1U);
}
