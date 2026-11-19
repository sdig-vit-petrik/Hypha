// The node owns every cycle of the session, below the C ABI. A cycle keeps running
// while the session lives, even after the Node handle it was started through is
// destroyed, and session shutdown destroys it once.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"

#include <gtest/gtest.h>

#include <atomic>
#include <memory>
#include <optional>
#include <string>

namespace {

/// The step counts its calls in storage that the test keeps.
struct CountingStep
{
    std::shared_ptr<std::atomic<int>> calls;

    void operator()(const hypha::CycleTime&) const
    {
        this->calls->fetch_add(1);
    }
};

}  // namespace

TEST(CycleRegistryLifetime, aCycleStartedThroughADefaultNodeKeepsRunningAfterThatHandleIsDestroyed)
{
    hypha::Node  owner("registry_lifetime_owner");
    auto         calls = std::make_shared<std::atomic<int>>(0);
    hypha::Cycle cycle("registry_lifetime_cycle");
    cycle.setRate(hypha::Hz(200.0));
    // Another test in this binary can leave a simulated clock installed for the
    // process. Its framework time does not move, so no cycle fires. Restore the
    // monotonic clock first, as the runner tests do.
    ASSERT_TRUE(hypha::disableSimTime());
    auto handle = std::make_unique<hypha::Node>();
    ASSERT_TRUE(cycle.start(*handle, CountingStep{calls}).has_value());

    ASSERT_TRUE(owner.runFor(hypha::Duration::fromNanos(100000000)).ok());
    ASSERT_GT(calls->load(), 0);

    handle.reset();
    const int beforeRun = calls->load();
    ASSERT_TRUE(owner.runFor(hypha::Duration::fromNanos(100000000)).ok());
    EXPECT_GT(calls->load(), beforeRun);
}

namespace {

/// The step keeps `lifetime`. The deleter of that storage counts each release.
struct OrphanStep
{
    std::shared_ptr<int> lifetime;

    void operator()(const hypha::CycleTime&) const
    {
    }
};

struct ReleaseCounter
{
    int* releases;

    void operator()(int* value) const
    {
        ++*this->releases;
        delete value;
    }
};

}  // namespace

TEST(CycleRegistryLifetime, aCycleOrphanedByADefaultNodeIsDestroyedOnceAtSessionShutdown)
{
    auto                 owner = std::make_unique<hypha::Node>("registry_orphan_owner");
    int                  releases = 0;
    std::shared_ptr<int> lifetime(new int(0), ReleaseCounter{&releases});
    hypha::Cycle         cycle("registry_orphan_cycle");
    cycle.setRate(hypha::Hz(200.0));
    ASSERT_TRUE(hypha::disableSimTime());
    auto handle = std::make_unique<hypha::Node>();
    ASSERT_TRUE(cycle.start(*handle, OrphanStep{lifetime}).has_value());
    lifetime.reset();

    handle.reset();
    ASSERT_TRUE(owner->runFor(hypha::Duration::fromNanos(100000000)).ok());
    EXPECT_EQ(releases, 0);

    owner.reset();
    EXPECT_EQ(releases, 1);
}

namespace {

/// The step is the only owner of a Node that the test drops before shutdown.
struct NodeHoldingStep
{
    std::shared_ptr<int>         lifetime;
    std::shared_ptr<hypha::Node> held;

    void operator()(const hypha::CycleTime&) const
    {
    }
};

}  // namespace

TEST(CycleRegistryLifetime, aNodeDestroyedDuringShutdownHandsItsCyclesToAnotherReleasePass)
{
    auto         owner = std::make_unique<hypha::Node>("registry_reentrant_owner");
    int          releases = 0;
    auto         detached = std::make_shared<hypha::Node>();
    hypha::Cycle inner("registry_reentrant_inner");
    inner.setRate(hypha::Hz(200.0));
    std::shared_ptr<int> innerLifetime(new int(0), ReleaseCounter{&releases});
    ASSERT_TRUE(inner.start(*detached, OrphanStep{innerLifetime}).has_value());
    innerLifetime.reset();

    hypha::Cycle outer("registry_reentrant_outer");
    outer.setRate(hypha::Hz(200.0));
    std::shared_ptr<int> outerLifetime(new int(0), ReleaseCounter{&releases});
    ASSERT_TRUE(outer.start(*owner, NodeHoldingStep{outerLifetime, detached}).has_value());
    outerLifetime.reset();
    detached.reset();

    // Releasing outer destroys its step, which destroys the detached Node. That Node's
    // inner cycle is released by the same shutdown: the node owns both.
    owner.reset();
    EXPECT_EQ(releases, 2);
}

TEST(CycleRegistryLifetime, aCycleObjectThatOutlivesTheSessionReleasesNothingTwice)
{
    int          releases = 0;
    hypha::Cycle cycle("registry_outlives_cycle");
    cycle.setRate(hypha::Hz(200.0));
    ASSERT_TRUE(hypha::disableSimTime());
    {
        hypha::Node          owner("registry_outlives_owner");
        std::shared_ptr<int> lifetime(new int(0), ReleaseCounter{&releases});
        ASSERT_TRUE(cycle.start(owner, OrphanStep{lifetime}).has_value());
        lifetime.reset();
        ASSERT_TRUE(cycle.health().has_value());
    }
    // The session destroyed the cycle. The Cycle object is destroyed after this
    // test body returns, and finds no cycle of its own to release.
    EXPECT_EQ(releases, 1);
    EXPECT_FALSE(cycle.health().has_value());
}
