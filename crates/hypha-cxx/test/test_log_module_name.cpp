// A valid module name compiles and registers. An invalid name is refused at
// compile time: see log_module_name_invalid.cpp, which a CTest must fail to build.

#include <hypha/metrics.h>
#include <hypha_log/hypha_log.h>

#include <gtest/gtest.h>

#include <optional>
#include <string>

HYPHA_LOG_MODULE_INIT("test/log_module_name_Valid_09")

static_assert(hypha::log::detail::isValidModuleName("a"));
static_assert(hypha::log::detail::isValidModuleName("scan_cycle"));
static_assert(hypha::log::detail::isValidModuleName("hypha/transport"));
static_assert(hypha::log::detail::isValidModuleName(
    "0123456789012345678901234567890123456789012345678901234567890123"
));
static_assert(!hypha::log::detail::isValidModuleName(""));
static_assert(!hypha::log::detail::isValidModuleName("has space"));
static_assert(!hypha::log::detail::isValidModuleName("dash-name"));
static_assert(!hypha::log::detail::isValidModuleName("dot.name"));
static_assert(!hypha::log::detail::isValidModuleName(
    "01234567890123456789012345678901234567890123456789012345678901234"
));

TEST(LogModuleName, aValidNameRegistersAndALineWrites)
{
    const std::optional<uint64_t> before = hypha::metric("log.lines_written");
    HYPHA_LOG_ERROR("module {}", "test/log_module_name_Valid_09");
    const std::optional<uint64_t> after = hypha::metric("log.lines_written");
    ASSERT_TRUE(before.has_value());
    ASSERT_TRUE(after.has_value());
    EXPECT_EQ(*after - *before, 1U);
}

TEST(LogModuleName, theSameNameGivesTheSameModuleIdentifier)
{
    const std::string name = "test/log_module_name_Valid_09";
    HyphaLogModule    first = 0;
    HyphaLogModule    second = 0;
    ASSERT_EQ(hypha_log_module_register(name.data(), name.size(), &first), HYPHA_STATUS_OK);
    ASSERT_EQ(hypha_log_module_register(name.data(), name.size(), &second), HYPHA_STATUS_OK);
    EXPECT_EQ(first, second);
}
