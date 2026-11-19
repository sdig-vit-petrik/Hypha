// The handle owner releases each handle once, in the order the SDK relies on.

#include "hypha/backend/handles.h"
#include "hypha/backend/text_readers.h"

#include <gtest/gtest.h>

#include <type_traits>
#include <utility>
#include <vector>

namespace {

struct FakeHandle
{
    int id;
};

std::vector<int> releasedIds;

}  // namespace

namespace hypha::backend {

template <>
struct HandleRelease<FakeHandle>
{
    static void release(FakeHandle* handle)
    {
        releasedIds.push_back(handle->id);
    }
};

}  // namespace hypha::backend

namespace {

using FakeOwner = hypha::backend::UniqueHandle<FakeHandle>;

static_assert(!std::is_copy_constructible_v<FakeOwner>);
static_assert(!std::is_copy_assignable_v<FakeOwner>);
static_assert(std::is_nothrow_move_constructible_v<FakeOwner>);
static_assert(std::is_nothrow_move_assignable_v<FakeOwner>);

class UniqueHandleTest : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        releasedIds.clear();
    }

    FakeHandle first{1};
    FakeHandle second{2};
};

TEST_F(UniqueHandleTest, DestructorReleasesTheHeldHandleOnce)
{
    {
        FakeOwner owner(&this->first);
        EXPECT_TRUE(static_cast<bool>(owner));
        EXPECT_EQ(owner.get(), &this->first);
    }
    EXPECT_EQ(releasedIds, std::vector<int>{1});
}

TEST_F(UniqueHandleTest, AnEmptyOwnerReleasesNothing)
{
    {
        FakeOwner owner;
        EXPECT_FALSE(static_cast<bool>(owner));
        EXPECT_EQ(owner.get(), nullptr);
    }
    EXPECT_TRUE(releasedIds.empty());
}

TEST_F(UniqueHandleTest, MoveConstructionTransfersOwnership)
{
    {
        FakeOwner source(&this->first);
        FakeOwner target(std::move(source));
        EXPECT_FALSE(static_cast<bool>(source));
        EXPECT_EQ(target.get(), &this->first);
        EXPECT_TRUE(releasedIds.empty());
    }
    EXPECT_EQ(releasedIds, std::vector<int>{1});
}

TEST_F(UniqueHandleTest, MoveAssignmentReleasesTheOldHandleFirst)
{
    {
        FakeOwner source(&this->second);
        FakeOwner target(&this->first);
        target = std::move(source);
        EXPECT_EQ(releasedIds, std::vector<int>{1});
        EXPECT_EQ(target.get(), &this->second);
    }
    EXPECT_EQ(releasedIds, (std::vector<int>{1, 2}));
}

TEST_F(UniqueHandleTest, ReleaseGivesUpOwnershipWithoutReleasing)
{
    {
        FakeOwner   owner(&this->first);
        FakeHandle* given = owner.release();
        EXPECT_EQ(given, &this->first);
        EXPECT_FALSE(static_cast<bool>(owner));
    }
    EXPECT_TRUE(releasedIds.empty());
}

TEST_F(UniqueHandleTest, ResetReleasesTheOldHandleAndHoldsTheNext)
{
    FakeOwner owner(&this->first);
    owner.reset(&this->second);
    EXPECT_EQ(releasedIds, std::vector<int>{1});
    EXPECT_EQ(owner.get(), &this->second);
    owner.reset();
    EXPECT_EQ(releasedIds, (std::vector<int>{1, 2}));
    EXPECT_FALSE(static_cast<bool>(owner));
}

TEST_F(UniqueHandleTest, PutReleasesTheOldHandleAndExposesTheSlot)
{
    FakeOwner    owner(&this->first);
    FakeHandle** slot = owner.put();
    EXPECT_EQ(releasedIds, std::vector<int>{1});
    ASSERT_NE(slot, nullptr);
    EXPECT_EQ(*slot, nullptr);
    *slot = &this->second;
    EXPECT_EQ(owner.get(), &this->second);
    owner.release();
}

TEST(TextReaders, ANullHandleIsRefusedWithTheStatusOfTheCAbi)
{
    const auto topic = hypha::backend::readSubscriberTopic(nullptr);
    ASSERT_FALSE(topic.has_value());
    EXPECT_EQ(topic.error().code, hypha::Status::InvalidArgument);

    const auto name = hypha::backend::readNodeName(nullptr);
    ASSERT_FALSE(name.has_value());
    EXPECT_EQ(name.error().code, hypha::Status::InvalidArgument);

    size_t length = 0;
    EXPECT_EQ(::hypha_node_introspect(nullptr, nullptr, 0, &length), HYPHA_STATUS_INVALID_ARGUMENT);
}

}  // namespace
