// Handle replacement must remove the executor registration it gives up, or the next run
// reads freed memory. A cycle that outlives the node must then be destroyed without
// touching the released WaitSet.

#include "hypha/cycle_node.h"

#include "test_types.h"

#include <gtest/gtest.h>

#include <memory>
#include <optional>
#include <string>
#include <type_traits>
#include <vector>

using hypha_test::advance;
using hypha_test::Reading;
using hypha_test::testTopic;
using namespace hypha::literals;

namespace {

class HandleLifetimeFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
    }

    std::optional<hypha::Node> node;
};

struct CountingCallback
{
    int* count;

    void operator()() const
    {
        *this->count += 1;
    }
};

struct ReadingCollector
{
    std::vector<int>* values;

    void operator()(const hypha::MessagePtr<Reading>& message, uint64_t) const
    {
        this->values->push_back(message->value);
    }
};

static_assert(!std::is_copy_assignable_v<hypha::backend::TimerHandle>);
static_assert(std::is_move_assignable_v<hypha::backend::TimerHandle>);
static_assert(!std::is_copy_assignable_v<hypha::backend::SubscriberHandle>);
static_assert(std::is_move_assignable_v<hypha::backend::SubscriberHandle>);

TEST_F(HandleLifetimeFixture, ReplacedTimerHandleStopsItsOwnCallback)
{
    this->node.emplace("handle_lifetime");

    int                         replacedCount = 0;
    int                         survivingCount = 0;
    hypha::backend::TimerHandle replaced =
        hypha::require(hypha::backend::createTimer(100_ms, CountingCallback{&replacedCount}));
    hypha::backend::TimerHandle surviving =
        hypha::require(hypha::backend::createTimer(100_ms, CountingCallback{&survivingCount}));
    ASSERT_TRUE(replaced.isValid());
    ASSERT_TRUE(surviving.isValid());

    advance(150_ms);
    ASSERT_TRUE(hypha_test::runUntil(*node, [&] {
        return survivingCount >= 1;
    }));
    ASSERT_EQ(replacedCount, 1);
    ASSERT_EQ(survivingCount, 1);

    // The replaced timer's state is released here. Its registration must go
    // with it, so the executor never reaches the freed callback again.
    replaced = std::move(surviving);

    advance(150_ms);
    ASSERT_TRUE(hypha_test::runUntil(*node, [&] {
        return survivingCount >= 2;
    }));
    EXPECT_EQ(replacedCount, 1);
    EXPECT_EQ(survivingCount, 2);
}

TEST_F(HandleLifetimeFixture, TimerHandleReplacedByAnEmptyHandleStops)
{
    this->node.emplace("handle_lifetime");

    int  fired = 0;
    auto timer = hypha::require(hypha::backend::createTimer(100_ms, CountingCallback{&fired}));
    ASSERT_TRUE(timer.isValid());

    advance(150_ms);
    ASSERT_TRUE(hypha_test::runUntil(*node, [&] {
        return fired >= 1;
    }));
    ASSERT_EQ(fired, 1);

    timer = hypha::backend::TimerHandle();
    EXPECT_FALSE(timer.isValid());

    advance(150_ms);
    // A released timer must stay silent. Give the host several turns to prove it.
    for (int turn = 0; turn < 5; ++turn) {
        ASSERT_TRUE(node->runFor(hypha::Duration::fromMillis(1)).ok());
    }
    EXPECT_EQ(fired, 1);
}

TEST_F(HandleLifetimeFixture, SelfMoveLeavesATimerHandleUsable)
{
    this->node.emplace("handle_lifetime");

    int  fired = 0;
    auto timer = hypha::require(hypha::backend::createTimer(100_ms, CountingCallback{&fired}));
    ASSERT_TRUE(timer.isValid());

    hypha::backend::TimerHandle& alias = timer;
    timer = std::move(alias);
    EXPECT_TRUE(timer.isValid());

    advance(150_ms);
    ASSERT_TRUE(hypha_test::runUntil(*node, [&] {
        return fired >= 1;
    }));
    EXPECT_EQ(fired, 1);
}

TEST_F(HandleLifetimeFixture, RepeatedTimerHandleReplacementLeavesOneRegistration)
{
    this->node.emplace("handle_lifetime");

    int  fired = 0;
    auto timer = hypha::require(hypha::backend::createTimer(100_ms, CountingCallback{&fired}));

    for (int round = 0; round < 4; ++round) {
        timer = hypha::require(hypha::backend::createTimer(100_ms, CountingCallback{&fired}));
    }

    advance(150_ms);
    ASSERT_TRUE(hypha_test::runUntil(*node, [&] {
        return fired >= 1;
    }));
    EXPECT_EQ(fired, 1);
}

struct ReplacesItsOwnTimerHandle
{
    hypha::backend::TimerHandle* handle;
    int*                         count;

    void operator()() const
    {
        *this->count += 1;
        *this->handle = hypha::backend::TimerHandle();
    }
};

// The WaitSet is running, so the executor refuses the detach. The timer state
// must outlive the handle until the registration is removed. Later periods
// must not call the released callback. The first fire is awaited in a loop,
// because one run does not always reach the due timer.
TEST_F(HandleLifetimeFixture, TimerHandleReplacedInsideItsOwnCallbackNeverFiresAgain)
{
    this->node.emplace("handle_lifetime");

    int                         fired = 0;
    hypha::backend::TimerHandle timer;
    timer = hypha::require(
        hypha::backend::createTimer(100_ms, ReplacesItsOwnTimerHandle{&timer, &fired})
    );
    ASSERT_TRUE(timer.isValid());

    constexpr int MAX_STEPS = 200;
    constexpr int LATER_PERIODS = 20;
    for (int step = 0; step < MAX_STEPS && fired == 0; ++step) {
        advance(150_ms);
        EXPECT_TRUE(node->runFor(1_ms).ok());
    }
    ASSERT_EQ(fired, 1);
    EXPECT_FALSE(timer.isValid());

    for (int period = 0; period < LATER_PERIODS; ++period) {
        advance(150_ms);
        EXPECT_TRUE(node->runFor(1_ms).ok());
    }
    EXPECT_EQ(fired, 1) << "a released timer must not run its callback again";
}

TEST_F(HandleLifetimeFixture, ReplacedSubscriberHandleStopsItsOwnCallback)
{
    this->node.emplace("handle_lifetime");

    const std::string topic = testTopic("handle_lifetime_sub");
    auto              publisher = hypha::Node().advertise<Reading>(hypha::Topic{topic});

    std::vector<int>                 replacedValues;
    std::vector<int>                 survivingValues;
    hypha::backend::SubscriberHandle replaced =
        hypha::backend::subscribeWithQos<Reading>(
            topic, hypha::backend::defaultSubscriberQos(8), ReadingCollector{&replacedValues}
        )
            .value();
    hypha::backend::SubscriberHandle surviving =
        hypha::backend::subscribeWithQos<Reading>(
            topic, hypha::backend::defaultSubscriberQos(8), ReadingCollector{&survivingValues}
        )
            .value();
    ASSERT_TRUE(replaced.isValid());
    ASSERT_TRUE(surviving.isValid());

    ASSERT_TRUE(publisher.publish(Reading{1, "first"}));
    ASSERT_TRUE(hypha_test::runUntil(*node, [&] {
        return survivingValues.size() >= 1;
    }));
    ASSERT_EQ(replacedValues.size(), 1U);
    ASSERT_EQ(survivingValues.size(), 1U);

    replaced = std::move(surviving);

    ASSERT_TRUE(publisher.publish(Reading{2, "second"}));
    ASSERT_TRUE(hypha_test::runUntil(*node, [&] {
        return survivingValues.size() >= 2;
    }));
    EXPECT_EQ(replacedValues.size(), 1U);
    EXPECT_EQ(survivingValues.size(), 2U);
}

TEST_F(HandleLifetimeFixture, SubscriberHandleReplacedByAnEmptyHandleStops)
{
    this->node.emplace("handle_lifetime");

    const std::string topic = testTopic("handle_lifetime_empty");
    auto              publisher = hypha::Node().advertise<Reading>(hypha::Topic{topic});

    std::vector<int>                 values;
    hypha::backend::SubscriberHandle subscriber =
        hypha::backend::subscribeWithQos<Reading>(
            topic, hypha::backend::defaultSubscriberQos(8), ReadingCollector{&values}
        )
            .value();
    ASSERT_TRUE(subscriber.isValid());

    ASSERT_TRUE(publisher.publish(Reading{1, "first"}));
    ASSERT_TRUE(hypha_test::runUntil(*node, [&] {
        return values.size() >= 1;
    }));
    ASSERT_EQ(values.size(), 1U);

    subscriber = hypha::backend::SubscriberHandle();
    EXPECT_FALSE(subscriber.isValid());

    // The released subscriber must not take the second sample. Several turns give it the chance.
    ASSERT_TRUE(publisher.publish(Reading{2, "second"}));
    for (int turn = 0; turn < 5; ++turn) {
        ASSERT_TRUE(node->runFor(hypha::Duration::fromMillis(1)).ok());
    }
    EXPECT_EQ(values.size(), 1U);
}

/// A step that holds `token`, so a test can see when its cycle releases it.
struct TokenStep
{
    std::shared_ptr<int> token;

    void operator()(const hypha::CycleTime&) const
    {
    }
};

TEST_F(HandleLifetimeFixture, NodeReleasesEachCycleBeforeItsWaitSet)
{
    std::weak_ptr<int> stepToken;
    // The cycle outlives the node, so its destruction runs after the release.
    hypha::Cycle cycle("shutdown_order");
    cycle.setRate(hypha::Hz(10.0));
    {
        hypha::Node shutdownNode("cycle_shutdown_order");
        auto        token = std::make_shared<int>(0);
        stepToken = token;
        const auto registered = cycle.start(shutdownNode, TokenStep{std::move(token)});
        ASSERT_TRUE(registered.has_value()) << registered.error();
        ASSERT_FALSE(stepToken.expired()) << "a registered cycle holds its step";
    }
    EXPECT_TRUE(stepToken.expired())
        << "destroying the node must release the cycle while its WaitSet still exists";

    // No session is left, so the cycle reports itself as gone.
    const auto health = cycle.health();
    ASSERT_FALSE(health.has_value());
    EXPECT_EQ(health.error(), hypha::Status::InvalidState);
}

}  // namespace
