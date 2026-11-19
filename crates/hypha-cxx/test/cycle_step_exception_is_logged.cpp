// An exception that leaves a cycle step ends the run with Panic. Its text and the
// cycle name are logged once, before the run stops. A composed (L2) cycle and a
// CycleNode (L3) share the same runner, so both are covered.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"

#include "test_types.h"

#include <gtest/gtest.h>

#include <optional>
#include <stdexcept>
#include <string>

using namespace hypha::literals;

namespace {

const char* const kStepText = "step failure text under test";
const char* const kComposedName = "cycle_step_logged_l2";
const char* const kNodeName = "cycle_step_exception_is_logged";

size_t occurrences(const std::string& text, const std::string& pattern)
{
    size_t count = 0;
    size_t position = text.find(pattern);
    while (position != std::string::npos) {
        count += 1;
        position = text.find(pattern, position + pattern.size());
    }
    return count;
}

class CycleStepLogFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace(kNodeName);
    }

    std::optional<hypha::Node> node;
};

struct ThrowingStep
{
    void operator()(const hypha::CycleTime&) const
    {
        throw std::runtime_error(kStepText);
    }
};

class ThrowingNode final : public hypha::CycleNode
{
  public:
    void setup(hypha::Node&) override
    {
        this->setRate(hypha::Hz(10.0));
    }

    void step(const hypha::CycleTime&) override
    {
        throw std::runtime_error(kStepText);
    }
};

TEST_F(CycleStepLogFixture, AComposedStepExceptionIsLoggedOnceWithTheCycleName)
{
    hypha::Cycle cycle(kComposedName);
    cycle.setRate(hypha::Hz(10.0));
    ASSERT_TRUE(cycle.start(*node, ThrowingStep{}).has_value());

    hypha_test::advance(100_ms);
    ::testing::internal::CaptureStderr();
    const hypha::RunResult failed = node->runFor(1_ms);
    const std::string      diagnostics = ::testing::internal::GetCapturedStderr();

    EXPECT_FALSE(failed.ok());
    EXPECT_EQ(failed.reason(), hypha::StopReason::RuntimeFailed);
    EXPECT_EQ(failed.status(), hypha::Status::Panic);
    EXPECT_EQ(occurrences(diagnostics, kStepText), 1U) << diagnostics;
    EXPECT_NE(diagnostics.find(kComposedName), std::string::npos) << diagnostics;
}

TEST_F(CycleStepLogFixture, ACycleNodeStepExceptionIsLoggedOnceWithTheCycleName)
{
    ThrowingNode app;
    app.setup(*node);
    ASSERT_TRUE(app.start(*node).has_value());

    hypha_test::advance(100_ms);
    ::testing::internal::CaptureStderr();
    const hypha::RunResult failed = node->runFor(1_ms);
    const std::string      diagnostics = ::testing::internal::GetCapturedStderr();

    EXPECT_FALSE(failed.ok());
    EXPECT_EQ(failed.reason(), hypha::StopReason::RuntimeFailed);
    EXPECT_EQ(failed.status(), hypha::Status::Panic);
    EXPECT_EQ(occurrences(diagnostics, kStepText), 1U) << diagnostics;
    EXPECT_NE(diagnostics.find(kNodeName), std::string::npos) << diagnostics;
}

}  // namespace
