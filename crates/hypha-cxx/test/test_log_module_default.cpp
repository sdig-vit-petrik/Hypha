// A translation unit without HYPHA_LOG_MODULE_INIT writes to the module "default".

#include <hypha/metrics.h>
#include <hypha_log/hypha_log.h>

#include <gtest/gtest.h>

#include <optional>

TEST(LogModuleDefault, aTranslationUnitWithoutInitWritesALine)
{
    const std::optional<uint64_t> before = hypha::metric("log.lines_written");
    HYPHA_LOG_ERROR("line from the default module");
    const std::optional<uint64_t> after = hypha::metric("log.lines_written");
    ASSERT_TRUE(before.has_value());
    ASSERT_TRUE(after.has_value());
    EXPECT_EQ(*after - *before, 1U);
}
