// A line that no target takes does not evaluate its arguments.

#include <hypha/metrics.h>
#include <hypha_log/hypha_log.h>

#include <gtest/gtest.h>

#include <optional>

HYPHA_LOG_MODULE_INIT("test/log_lazy_arguments")

namespace {

int evaluations = 0;

int countedArgument()
{
    ++evaluations;
    return evaluations;
}

}  // namespace

TEST(LogLazyArguments, aDisabledLineDoesNotEvaluateItsArguments)
{
    // The default module level is debug: trace is disabled.
    evaluations = 0;
    HYPHA_LOG_TRACE("value {}", countedArgument());
    HYPHA_LOG_TRACE_THROTTLE(1.0, "value {}", countedArgument());
    HYPHA_LOG_TRACE_WHEN(true, false, "value {}", countedArgument());
    HYPHA_LOG_TRACE_WHEN_THROTTLE(true, false, 1.0, "value {}", countedArgument());
    EXPECT_EQ(evaluations, 0);
}

TEST(LogLazyArguments, anEnabledLineEvaluatesItsArgumentsOnce)
{
    evaluations = 0;
    HYPHA_LOG_DEBUG("value {}", countedArgument());
    EXPECT_EQ(evaluations, 1);
}

TEST(LogLazyArguments, aThrottledLineEvaluatesItsArgumentsOnlyWhenItWrites)
{
    evaluations = 0;
    for (int call = 0; call < 4; ++call) {
        HYPHA_LOG_ERROR_THROTTLE(3600.0, "value {}", countedArgument());
    }
    EXPECT_EQ(evaluations, 1);
}

TEST(LogLazyArguments, aLineWithoutArgumentsWrites)
{
    const std::optional<uint64_t> before = hypha::metric("log.lines_written");
    HYPHA_LOG_ERROR("plain text with {{braces}}");
    const std::optional<uint64_t> after = hypha::metric("log.lines_written");
    ASSERT_TRUE(before.has_value());
    ASSERT_TRUE(after.has_value());
    EXPECT_EQ(*after - *before, 1U);
}
