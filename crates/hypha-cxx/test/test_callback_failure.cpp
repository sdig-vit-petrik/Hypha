// An uncaught exception at a C++ callback boundary fails the owning run with
// Panic. Later queued work must not execute, and callback contexts are released
// when their registration is detached.

#include "test_types.h"

#include <gtest/gtest.h>

#include <memory>
#include <optional>
#include <stdexcept>
#include <string>

using hypha_test::Reading;
using hypha_test::testTopic;
using namespace hypha::literals;

namespace {

class CallbackFailureFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("callback_failure");
    }

    std::optional<hypha::Node> node;
};

/// Counts each call, then throws.
struct ThrowingReader
{
    int*                 calls;
    std::shared_ptr<int> lifetime;

    void operator()(const hypha::MessagePtr<Reading>&) const
    {
        *this->calls += 1;
        throw std::runtime_error("reader failure under test");
    }
};

/// Counts each call, then throws.
struct ThrowingTick
{
    int*                 calls;
    std::shared_ptr<int> lifetime;

    void operator()() const
    {
        *this->calls += 1;
        (void)this->lifetime;
        throw std::runtime_error("tick failure under test");
    }
};

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

TEST_F(CallbackFailureFixture, AThrowingSubscriptionFailsTheRunBeforeLaterSamples)
{
    const std::string  topic = testTopic("/throwing_reader");
    int                calls = 0;
    auto               publisher = hypha::Node().advertise<Reading>(hypha::Topic{topic});
    auto               lifetime = std::make_shared<int>(1);
    std::weak_ptr<int> weakLifetime = lifetime;
    auto               subscriber =
        hypha::Node().subscribe<Reading>(hypha::Topic{topic}, ThrowingReader{&calls, lifetime});
    ASSERT_TRUE(publisher.enabled());
    ASSERT_TRUE(subscriber.enabled());
    lifetime.reset();

    ASSERT_TRUE(publisher.publish(Reading{1, "one"}));
    ASSERT_TRUE(publisher.publish(Reading{2, "two"}));
    ASSERT_TRUE(publisher.publish(Reading{3, "three"}));

    ::testing::internal::CaptureStderr();
    const hypha::RunResult result = node->runFor(20_ms);
    const std::string      diagnostics = ::testing::internal::GetCapturedStderr();

    EXPECT_FALSE(result.ok());
    EXPECT_EQ(result.reason(), hypha::StopReason::RuntimeFailed);
    EXPECT_EQ(result.status(), hypha::Status::Panic);
    EXPECT_EQ(calls, 1) << "failure stops this run before later queued samples";
    EXPECT_EQ(occurrences(diagnostics, "run failed"), 1U) << diagnostics;
    EXPECT_EQ(hypha::metric("node.callbacks_invoked"), 1U);
    EXPECT_EQ(hypha::metric("node.callback_failures"), 1U);
    EXPECT_EQ(
        hypha::metric("node.last_callback_status"), static_cast<uint64_t>(hypha::Status::Panic)
    );

    subscriber = {};
    publisher = {};
    this->node.reset();
    EXPECT_TRUE(weakLifetime.expired());
}

TEST_F(CallbackFailureFixture, AThrowingTimerFailsTheRunAtItsFirstFire)
{
    int                calls = 0;
    auto               lifetime = std::make_shared<int>(1);
    std::weak_ptr<int> weakLifetime = lifetime;
    hypha::Timer       timer = node->timer(100_ms, ThrowingTick{&calls, lifetime});
    ASSERT_TRUE(timer.isValid());
    lifetime.reset();

    ::testing::internal::CaptureStderr();
    hypha_test::advance(150_ms);
    const hypha::RunResult result = node->runFor(1_ms);
    const std::string      diagnostics = ::testing::internal::GetCapturedStderr();

    EXPECT_FALSE(result.ok());
    EXPECT_EQ(result.reason(), hypha::StopReason::RuntimeFailed);
    EXPECT_EQ(result.status(), hypha::Status::Panic);
    EXPECT_EQ(calls, 1) << "the timer callback is terminal after it throws";
    EXPECT_EQ(occurrences(diagnostics, "run failed"), 1U) << diagnostics;
    EXPECT_EQ(hypha::metric("node.callback_failures"), 1U);
    timer = {};
    EXPECT_TRUE(weakLifetime.expired());
}

}  // namespace
