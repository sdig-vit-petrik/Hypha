// A translation unit that selects a log module and never logs must compile
// under -Werror: the module function the macro defines is not dead code.

#include <hypha_log/hypha_log.h>

#include <gtest/gtest.h>

HYPHA_LOG_MODULE_INIT("test/log_module_without_calls")

TEST(LogModuleWithoutCalls, aTranslationUnitWithoutLogCallsCompiles)
{
    SUCCEED();
}
