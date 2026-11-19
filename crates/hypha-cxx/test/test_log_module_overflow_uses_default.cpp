// Past 256 modules, a new module name writes under the module "default" instead
// of aborting the process. Its own executable: the module table is process-wide.

#include <hypha/metrics.h>
#include <hypha_log/hypha_log.h>

#include <gtest/gtest.h>

#include <optional>
#include <string>

TEST(LogModuleOverflow, aModulePast256DoesNotAbortAndUsesTheDefaultModule)
{
    HyphaLogModule last = 1;
    for (int index = 0; index < 300; ++index) {
        const std::string name = "overflow_" + std::to_string(index);
        last                   = hypha::log::detail::registerModule(name);
    }
    EXPECT_EQ(last, 0U) << "a name past the limit gets the identifier of \"default\"";
    const std::optional<uint64_t> before = hypha::metric("log.lines_written");
    hypha::log::detail::write(last, HYPHA_LOG_LEVEL_ERROR, "line past the module limit");
    const std::optional<uint64_t> after = hypha::metric("log.lines_written");
    ASSERT_TRUE(before.has_value());
    ASSERT_TRUE(after.has_value());
    EXPECT_EQ(*after - *before, 1U);
}
