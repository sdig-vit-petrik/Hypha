/// @file
/// The names of the result enums: every enumerator by its own name, "Unknown" for a value
/// outside the enum, as Status::name() gives.

#include <hypha/cycle.h>
#include <hypha/raw.h>
#include <hypha/transform_types.h>

#include <gtest/gtest.h>

#include <cstdint>

TEST(ResultNames, EveryTransformErrorHasItsName)
{
    EXPECT_EQ(hypha::name(hypha::TransformError::InvalidFrame), "InvalidFrame");
    EXPECT_EQ(hypha::name(hypha::TransformError::UnknownFrame), "UnknownFrame");
    EXPECT_EQ(hypha::name(hypha::TransformError::NotConnected), "NotConnected");
    EXPECT_EQ(hypha::name(hypha::TransformError::BeforeBuffer), "BeforeBuffer");
    EXPECT_EQ(hypha::name(hypha::TransformError::AfterBuffer), "AfterBuffer");
    EXPECT_EQ(hypha::name(static_cast<hypha::TransformError>(99)), "Unknown");
}

TEST(ResultNames, EveryInputVerdictHasItsName)
{
    EXPECT_EQ(hypha::name(hypha::InputVerdict::NeverArrived), "NeverArrived");
    EXPECT_EQ(hypha::name(hypha::InputVerdict::Usable), "Usable");
    EXPECT_EQ(hypha::name(hypha::InputVerdict::Stale), "Stale");
    EXPECT_EQ(hypha::name(static_cast<hypha::InputVerdict>(uint32_t{7})), "Unknown");
}

TEST(ResultNames, EveryStopReasonHasItsName)
{
    EXPECT_EQ(hypha::name(hypha::StopReason::Requested), "Requested");
    EXPECT_EQ(hypha::name(hypha::StopReason::SetupFailed), "SetupFailed");
    EXPECT_EQ(hypha::name(hypha::StopReason::RuntimeFailed), "RuntimeFailed");
    EXPECT_EQ(hypha::name(hypha::StopReason::Deadline), "Deadline");
    EXPECT_EQ(hypha::name(static_cast<hypha::StopReason>(uint8_t{9})), "Unknown");
}

// The names are constant expressions, as Status::name() is.
static_assert(hypha::name(hypha::TransformError::AfterBuffer) == "AfterBuffer");
static_assert(hypha::name(hypha::InputVerdict::Stale) == "Stale");
static_assert(hypha::name(hypha::StopReason::Deadline) == "Deadline");
