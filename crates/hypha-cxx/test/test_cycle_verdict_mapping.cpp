// The C++ facade maps each verdict the library reports, and refuses any other value.

#include "hypha/cycle.h"
#include "hypha/hypha.h"

#include <gtest/gtest.h>

#include <cstdint>

namespace {

using hypha::InputVerdict;

TEST(CycleVerdictMapping, everyKnownVerdictMapsToItsValue)
{
    const auto map = [](int32_t raw) {
        return hypha::detail::verdictFromRaw(raw).value();
    };
    EXPECT_EQ(map(HYPHA_CYCLE_VERDICT_USABLE), InputVerdict::Usable);
    EXPECT_EQ(map(HYPHA_CYCLE_VERDICT_NEVER_ARRIVED), InputVerdict::NeverArrived);
    EXPECT_EQ(map(HYPHA_CYCLE_VERDICT_STALE), InputVerdict::Stale);
}

TEST(CycleVerdictMapping, anUnknownVerdictIsRefused)
{
    const auto unknown = hypha::detail::verdictFromRaw(HYPHA_CYCLE_VERDICT_STALE + 1);
    ASSERT_FALSE(unknown.has_value());
    EXPECT_EQ(unknown.error(), HYPHA_STATUS_INVALID_STATE);
    EXPECT_FALSE(hypha::detail::verdictFromRaw(-1).has_value());
}

}  // namespace
