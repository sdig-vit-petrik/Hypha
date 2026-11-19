// Framework time that moves backwards, seen through the C++ SDK.
//
// A recording replayed again, or a replay that seeks back, moves `/clock`
// backwards. A timer then ends the run with ClockRewound.

#include "test_types.h"

#include <gtest/gtest.h>

#include <optional>
#include <string>

using namespace hypha::literals;

namespace {

constexpr int64_t RECORDING_START_NANOS = 1000000000000;
constexpr int64_t REPLAY_START_NANOS = 5000000000;

/// Counts timer calls into a counter the test owns.
struct CountFire
{
    int* fired;

    void operator()() const
    {
        *this->fired += 1;
    }
};

/// Moves simulated time forward by `jump` while the callback runs.
struct JumpDuringCallback
{
    hypha::Duration jump;
    int*            calls;

    void operator()(const hypha::Subscriber<hypha_test::Reading>::ConstPtr& /*message*/) const
    {
        hypha_test::advance(this->jump);
        *this->calls += 1;
    }
};

class TimeJumpFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(RECORDING_START_NANOS)));
        this->node.emplace("time_jump");
    }

    void TearDown() override
    {
        this->node.reset();
        EXPECT_TRUE(hypha::disableSimTime());
    }

    std::optional<hypha::Node> node;
};

TEST_F(TimeJumpFixture, ARewindEndsTheRunWithClockRewound)
{
    int                fired = 0;
    const hypha::Timer timer = node->timer(100_ms, CountFire{&fired});
    ASSERT_TRUE(timer.isValid());

    hypha_test::advance(150_ms);
    ASSERT_TRUE(hypha_test::runUntilSweepAt(*node, hypha::now().nanos()));
    ASSERT_EQ(fired, 1);

    // Nothing rewinds in place: the timer that sees simulated time go back ends the run, and
    // runNode restarts the process.
    ASSERT_TRUE(hypha::setSimTime(hypha::Time::fromNanos(REPLAY_START_NANOS)));
    const hypha::RunResult rewound = node->runFor(100_ms);
    EXPECT_FALSE(rewound.ok());
    EXPECT_EQ(rewound.status(), hypha::Status::ClockRewound);
    EXPECT_EQ(fired, 1) << "the rewind fires nothing";
}

TEST_F(TimeJumpFixture, ASimulatedTimeJumpDuringACallbackIsNotASlowCallback)
{
    const std::string topic = hypha_test::testTopic("/jump_in_callback");
    auto              publisher = hypha::Node().advertise<hypha_test::Reading>(hypha::Topic{topic});
    int               calls = 0;
    auto              subscriber = hypha::Node().subscribe<hypha_test::Reading>(
        hypha::Topic{topic}, JumpDuringCallback{10_s, &calls}
    );
    ASSERT_TRUE(publisher.publish(hypha_test::Reading{1, "jump"}));

    ::testing::internal::CaptureStderr();
    ASSERT_TRUE(node->runFor(20_ms).ok());
    const std::string diagnostics = ::testing::internal::GetCapturedStderr();

    ASSERT_EQ(calls, 1);
    EXPECT_EQ(diagnostics.find("exceeded"), std::string::npos) << diagnostics;
    EXPECT_EQ(diagnostics.find("callback took"), std::string::npos) << diagnostics;
    EXPECT_EQ(hypha::metric("node.slow_callbacks"), 0U) << "a callback is timed on the host clock";
}

}  // namespace
