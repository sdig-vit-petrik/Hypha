// A library that reports another ABI revision must be refused before any node,
// configuration or logging call crosses the ABI.
// This executable defines its own hypha_abi_revision(), which takes precedence over
// libhypha_ffi, and counts calls to the startup entry points.

#include "hypha/application_node.h"
#include "hypha/backend/abi.h"
#include "hypha/config.h"
#include "hypha/hypha_ffi.h"
#include "hypha/node.h"

#include <gtest/gtest.h>

#include <stdexcept>
#include <string>

namespace {

int abiCallsPastTheCheck = 0;
int reportedRevision = 0;

}  // namespace

extern "C" {

uint32_t hypha_abi_revision(void)
{
    return static_cast<uint32_t>(reportedRevision);
}

HyphaStatus hypha_node_create(
    const char*, size_t, const char*, size_t, const struct HyphaConfig*, struct HyphaNode**
)
{
    ++abiCallsPastTheCheck;
    return HYPHA_STATUS_BACKEND;
}

HyphaStatus hypha_config_create(const char*, size_t, struct HyphaConfig**)
{
    ++abiCallsPastTheCheck;
    return HYPHA_STATUS_BACKEND;
}

HyphaStatus hypha_log_module_register(const char*, size_t, HyphaLogModule*)
{
    ++abiCallsPastTheCheck;
    return HYPHA_STATUS_BACKEND;
}

int32_t hypha_log_enabled(HyphaLogModule, uint32_t)
{
    ++abiCallsPastTheCheck;
    return 0;
}

void hypha_log_write(HyphaLogModule, uint32_t, const char*, size_t)
{
    ++abiCallsPastTheCheck;
}

}  // extern "C"

namespace {

bool applicationWasConstructed = false;

class UnreachableApp final : public hypha::ApplicationNode
{
  public:
    UnreachableApp()
    {
        applicationWasConstructed = true;
    }

    void setup(hypha::Node&) override
    {
    }
};

class AbiMismatch : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        abiCallsPastTheCheck = 0;
        applicationWasConstructed = false;
        reportedRevision = static_cast<int>(HYPHA_ABI_REVISION) + 1;
    }
};

TEST_F(AbiMismatch, TheDiagnosticNamesBothRevisions)
{
    std::string diagnostic;

    EXPECT_FALSE(hypha::backend::verifyAbi(&diagnostic));

    const std::string expected = "hypha ABI revision mismatch: SDK expects "
                               + std::to_string(HYPHA_ABI_REVISION) + ", library reports "
                               + std::to_string(reportedRevision);
    EXPECT_EQ(diagnostic, expected);
}

TEST_F(AbiMismatch, TheExactRevisionIsAccepted)
{
    reportedRevision = static_cast<int>(HYPHA_ABI_REVISION);

    EXPECT_TRUE(hypha::backend::verifyAbi());
}

TEST_F(AbiMismatch, AnOlderLibraryIsRefusedToo)
{
    reportedRevision = static_cast<int>(HYPHA_ABI_REVISION) - 1;

    EXPECT_FALSE(hypha::backend::verifyAbi());
}

TEST_F(AbiMismatch, NodeCreationIsRefusedBeforeAnyAbiCall)
{
    testing::internal::CaptureStderr();

    EXPECT_THROW(hypha::Node("abi_mismatch_node"), std::runtime_error);

    const std::string errors = testing::internal::GetCapturedStderr();
    EXPECT_NE(errors.find("ABI revision mismatch"), std::string::npos) << errors;
    EXPECT_EQ(abiCallsPastTheCheck, 0);
}

TEST_F(AbiMismatch, ConfigurationLoadingIsRefusedBeforeAnyAbiCall)
{
    testing::internal::CaptureStderr();

    EXPECT_THROW(hypha::ConfigContext(std::string{}), std::runtime_error);

    const std::string errors = testing::internal::GetCapturedStderr();
    EXPECT_NE(errors.find("ABI revision mismatch"), std::string::npos) << errors;
    EXPECT_EQ(abiCallsPastTheCheck, 0);
}

TEST_F(AbiMismatch, RunNodeReturnsTheConfigurationCodeBeforeItBuildsTheApplication)
{
    char  name[] = "abi_mismatch_run";
    char* arguments[] = {name};
    testing::internal::CaptureStderr();

    const int exitCode = hypha::runNode<UnreachableApp>(1, arguments, "abi_mismatch_run");

    const std::string errors = testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(errors.find("ABI revision mismatch"), std::string::npos) << errors;
    EXPECT_FALSE(applicationWasConstructed);
    EXPECT_EQ(abiCallsPastTheCheck, 0);
}

}  // namespace
