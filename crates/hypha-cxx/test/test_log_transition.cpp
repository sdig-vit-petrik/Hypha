// _WHEN writes on a false to true change. _WHEN_THROTTLE writes on the change,
// then at most once per period while the state stays true.

#include <hypha/metrics.h>
#include <hypha_log/hypha_log.h>

#include <gtest/gtest.h>

#include <chrono>
#include <optional>
#include <thread>

HYPHA_LOG_MODULE_INIT("test/log_transition")

namespace {

uint64_t linesWritten()
{
    const std::optional<uint64_t> lines = hypha::metric("log.lines_written");
    EXPECT_TRUE(lines.has_value());
    return lines.value_or(0);
}

uint64_t rejectedLines()
{
    const std::optional<uint64_t> rejected = hypha::metric("log.rejected_lines");
    EXPECT_TRUE(rejected.has_value());
    return rejected.value_or(0);
}

void whenFromFalse(bool state)
{
    HYPHA_LOG_ERROR_WHEN(state, false, "state is true");
}

void whenFromTrue(bool state)
{
    HYPHA_LOG_ERROR_WHEN(state, true, "state is true");
}

void whenThrottleFromFalse(bool state, double periodSeconds)
{
    HYPHA_LOG_ERROR_WHEN_THROTTLE(state, false, periodSeconds, "state is true");
}

void whenThrottleFromTrue(bool state, double periodSeconds)
{
    HYPHA_LOG_ERROR_WHEN_THROTTLE(state, true, periodSeconds, "state is true");
}

}  // namespace

TEST(LogTransition, whenWritesOnceOnTheRisingEdgeFromInitialFalse)
{
    const uint64_t before = linesWritten();
    whenFromFalse(false);
    whenFromFalse(false);
    EXPECT_EQ(linesWritten() - before, 0U);
    whenFromFalse(true);
    whenFromFalse(true);
    EXPECT_EQ(linesWritten() - before, 1U);
    whenFromFalse(false);
    whenFromFalse(true);
    EXPECT_EQ(linesWritten() - before, 2U);
}

TEST(LogTransition, whenWithInitialFalseWritesOnTheFirstCallThatIsTrue)
{
    const uint64_t before = linesWritten();
    HYPHA_LOG_ERROR_WHEN(true, false, "first call is true");
    EXPECT_EQ(linesWritten() - before, 1U);
}

TEST(LogTransition, whenWithInitialTrueDoesNotWriteWhileTheStateStaysTrue)
{
    const uint64_t before = linesWritten();
    whenFromTrue(true);
    whenFromTrue(true);
    EXPECT_EQ(linesWritten() - before, 0U);
    whenFromTrue(false);
    whenFromTrue(true);
    EXPECT_EQ(linesWritten() - before, 1U);
}

TEST(LogTransition, whenThrottleWritesOnTheEdgeAndThenOncePerPeriod)
{
    const uint64_t before = linesWritten();
    whenThrottleFromFalse(false, 0.05);
    EXPECT_EQ(linesWritten() - before, 0U);
    whenThrottleFromFalse(true, 0.05);
    whenThrottleFromFalse(true, 0.05);
    EXPECT_EQ(linesWritten() - before, 1U);
    std::this_thread::sleep_for(std::chrono::milliseconds(80));
    whenThrottleFromFalse(true, 0.05);
    EXPECT_EQ(linesWritten() - before, 2U);
}

TEST(LogTransition, whenThrottleWritesAgainOnANewEdgeInsideThePeriod)
{
    const uint64_t before = linesWritten();
    const bool     states[] = {true, false, true};
    for (const bool state : states) {
        HYPHA_LOG_ERROR_WHEN_THROTTLE(state, false, 3600.0, "edge");
    }
    EXPECT_EQ(linesWritten() - before, 2U);
}

TEST(LogTransition, whenThrottleWithInitialTrueWaitsOnePeriodBeforeTheFirstLine)
{
    const uint64_t before = linesWritten();
    whenThrottleFromTrue(true, 0.05);
    EXPECT_EQ(linesWritten() - before, 0U);
    std::this_thread::sleep_for(std::chrono::milliseconds(80));
    whenThrottleFromTrue(true, 0.05);
    EXPECT_EQ(linesWritten() - before, 1U);
}

TEST(LogTransition, whenThrottleWithAnInvalidPeriodSuppressesAndCounts)
{
    const uint64_t linesBefore = linesWritten();
    const uint64_t rejectedBefore = rejectedLines();
    HYPHA_LOG_ERROR_WHEN_THROTTLE(true, false, -1.0, "invalid period");
    EXPECT_EQ(linesWritten(), linesBefore);
    EXPECT_EQ(rejectedLines() - rejectedBefore, 1U);
}
