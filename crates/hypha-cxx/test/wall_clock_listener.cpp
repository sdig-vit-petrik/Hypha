/// @file
/// @brief Returns framework time to the wall clock after every test of test_hypha.

#include "hypha/hypha.h"

#include <gtest/gtest.h>

namespace {

/// A named functor, because gtest owns the listener until the process exits.
class WallClockAfterEachTest : public ::testing::EmptyTestEventListener
{
  public:
    void OnTestEnd(const ::testing::TestInfo& /*info*/) override
    {
        EXPECT_TRUE(hypha::disableSimTime());
    }
};

/// Registers the listener before main() runs, so it covers every test.
bool registerWallClockListener()
{
    ::testing::UnitTest::GetInstance()->listeners().Append(new WallClockAfterEachTest);
    return true;
}

[[maybe_unused]] const bool registered = registerWallClockListener();

}  // namespace
