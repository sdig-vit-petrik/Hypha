// Level 1, pure logic: the control law alone. No hypha header, no harness, no bus.

#include "scan_cycle/command_law.h"

#include <gtest/gtest.h>

#include <optional>

using scan_cycle::controlCommand;

TEST(ControlLaw, TheCommandIsTheSpeedErrorTimesTheGain)
{
    EXPECT_DOUBLE_EQ(controlCommand(4.0, 12.0, 10.0, 0.5), 4.0);
    EXPECT_DOUBLE_EQ(controlCommand(12.0, 4.0, 10.0, 0.5), -4.0);
}

TEST(ControlLaw, WithoutASetpointTheFallbackIsUsed)
{
    EXPECT_DOUBLE_EQ(controlCommand(4.0, std::nullopt, 10.0, 0.5), 3.0);
}

TEST(ControlLaw, WithoutASpeedNoMotionIsCommanded)
{
    EXPECT_DOUBLE_EQ(controlCommand(std::nullopt, 12.0, 10.0, 0.5), 0.0);
    EXPECT_DOUBLE_EQ(controlCommand(std::nullopt, std::nullopt, 10.0, 0.5), 0.0);
}
