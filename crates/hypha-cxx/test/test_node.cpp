#include "test_types.h"

#include <gtest/gtest.h>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <csignal>
#include <mutex>
#include <optional>
#include <string>
#include <thread>

using hypha_test::advance;
using hypha_test::Reading;
using hypha_test::testTopic;
using namespace hypha::literals;

namespace {

class NodeFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
    }
};

TEST_F(NodeFixture, ANodeLeavesTheBusWhenItGoesOutOfScope)
{
    {
        hypha::Node node("node_raii_test");
        EXPECT_TRUE(hypha::ok());
        EXPECT_EQ(hypha::nodeName(), "node_raii_test");
    }
    // The destructor released the session. Nothing else had to ask for it.
    EXPECT_FALSE(hypha::ok());
}

struct AsyncCounter
{
    std::atomic<int>*        count;
    std::mutex*              mutex;
    std::condition_variable* condition;

    void operator()(const hypha::MessagePtr<Reading>&) const
    {
        this->count->fetch_add(1, std::memory_order_release);
        const std::lock_guard<std::mutex> lock(*this->mutex);
        this->condition->notify_all();
    }
};

struct ReadingCounter
{
    int* count;

    void operator()(const hypha::MessagePtr<Reading>&) const
    {
        ++(*this->count);
    }
};

struct RawSampleFlag
{
    bool* seen;

    void operator()(const hypha::RawSample&) const
    {
        *this->seen = true;
    }
};

struct RawSampleCapture
{
    std::string*          topic;
    std::vector<uint8_t>* payload;
    bool*                 delivered;

    void operator()(const hypha::RawSample& sample) const
    {
        *this->topic = sample.topic;
        this->payload->assign(sample.bytes.begin(), sample.bytes.end());
        *this->delivered = true;
    }
};

}  // namespace

TEST_F(NodeFixture, NotOkBeforeANode)
{
    EXPECT_FALSE(hypha::ok());
}

TEST_F(NodeFixture, RunNodeRejectsExtraArgumentsBeforeInitialization)
{
    struct App final : hypha::ApplicationNode
    {
    };

    char  executable[] = "app_probe";
    char  parameterFile[] = "config.toml";
    char  extra[] = "--typo";
    char* argv[] = {executable, parameterFile, extra, nullptr};

    ::testing::internal::CaptureStderr();
    const int         result = hypha::runNode<App>(3, argv, "app_probe");
    const std::string diagnostic = ::testing::internal::GetCapturedStderr();

    EXPECT_EQ(result, 2);
    EXPECT_FALSE(hypha::ok());
    EXPECT_NE(diagnostic.find("usage: app_probe [parameter_file]"), std::string::npos);
}

/// A stop signal is for the rest of the process, so a test that raises one runs in a
/// process of its own (CMakeLists.txt: the `*StopSignalProcess` suites).
class NodeStopSignalProcess : public NodeFixture
{
};

TEST_F(NodeStopSignalProcess, StopSignalEndsTheRun)
{
    hypha::Node node("node_test");
    ASSERT_TRUE(hypha::ok());

    std::raise(SIGTERM);
    EXPECT_FALSE(hypha::ok());
}

TEST_F(NodeStopSignalProcess, ANewNodeAfterAStopSignalIsStoppedFromTheStart)
{
    {
        hypha::Node node("node_test");
        std::raise(SIGINT);
        ASSERT_FALSE(hypha::ok());
    }

    hypha::Node node("node_test");
    EXPECT_FALSE(hypha::ok());
}

TEST_F(NodeFixture, SecondNodeKeepsTheFirstName)
{
    hypha::Node first("first");
    hypha::Node second("second");
    EXPECT_EQ(hypha::nodeName(), "first");
}

TEST_F(NodeFixture, NodeInstallsTheFrameworkClock)
{
    hypha::Node node("node_test");

    const hypha::Time before = hypha::now();
    EXPECT_FALSE(before.isZero());

    advance(1_s);
    EXPECT_DOUBLE_EQ((hypha::now() - before).seconds(), 1.0);
}

TEST_F(NodeFixture, NodeReleaseKeepsAnExternallySelectedClock)
{
    {
        hypha::Node node("node_test");
        ASSERT_TRUE(hypha::isSimTime());
    }
    EXPECT_TRUE(hypha::isSimTime());
}

TEST_F(NodeFixture, TimerFiresWhenItsPeriodElapses)
{
    hypha::Node node("node_test");

    int  fired = 0;
    auto timer = node.timer(100_ms, [&] {
        ++fired;
    });
    ASSERT_TRUE(timer.isValid());

    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(fired, 0);

    advance(150_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(fired, 1);

    advance(150_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(fired, 2);
}

TEST_F(NodeFixture, TimerCountsThePeriodsItCouldNotFireIn)
{
    hypha::Node node("node_test");

    int  fired = 0;
    auto timer = node.timer(100_ms, [&] {
        ++fired;
    });

    advance(150_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    ASSERT_EQ(fired, 1);
    EXPECT_EQ(timer.missedCount(), 0U);

    // A whole second passes without a run: nine periods went by unserved, and
    // the timer reports them rather than firing nine times in a row.
    advance(1000_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(fired, 2);
    EXPECT_EQ(timer.missedCount(), 9U);
}

TEST_F(NodeFixture, TimerStopsWhenItGoesOutOfScope)
{
    hypha::Node node("node_test");

    int fired = 0;
    {
        auto timer = node.timer(100_ms, [&] {
            ++fired;
        });
        advance(150_ms);
        (void)node.runFor(hypha::Duration::fromMillis(1));
        ASSERT_EQ(fired, 1);
    }

    advance(150_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(fired, 1);
}

TEST_F(NodeFixture, NotifiedRunDrainsTheWholeQueue)
{
    hypha::Node node("node_test");

    int            received = 0;
    ReadingCounter callback{&received};
    auto           pub = hypha::Node().advertise<Reading>(hypha::Topic{testTopic("/wait_queue")});
    auto sub = hypha::Node().subscribe<Reading>(hypha::Topic{testTopic("/wait_queue")}, callback);

    ASSERT_TRUE(pub.publish(Reading{1, "one"}));
    ASSERT_TRUE(pub.publish(Reading{2, "two"}));
    ASSERT_TRUE(pub.publish(Reading{3, "three"}));
    (void)node.runFor(hypha::Duration::fromMillis(500));

    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(received, 3);
}

TEST_F(NodeFixture, TimerWithoutAPositivePeriodIsRefused)
{
    hypha::Node node("node_test");

    const std::string refusal = hypha_test::refusalOf([&node] {
        static_cast<void>(node.timer(hypha::Duration(), [] {
        }));
    });
    EXPECT_NE(refusal.find("InvalidArgument"), std::string::npos) << refusal;

    // A timer that does fire still bounds the wait by its own period.
    const hypha::Duration period = 20_ms;
    hypha::Timer          paced = node.timer(period, [] {
    });
    ASSERT_TRUE(paced.isValid());
}

TEST_F(NodeFixture, WildcardSubscriptionIsPolled)
{
    hypha::Node node("node_test");

    bool          seen = false;
    RawSampleFlag callback{&seen};

    // A pattern has no single event service behind it, so the library polls it.
    const auto polling = hypha::subscribeRaw(testTopic("/wildcard_notify/**"), callback);
    ASSERT_TRUE(polling.has_value());
    EXPECT_TRUE(polling->isValid());
}

TEST_F(NodeFixture, NotifiedSubscriberKeepsTheLatestState)
{
    hypha::Node node("node_test");

    auto pub = hypha::Node().advertise<Reading>(hypha::Topic{testTopic("/wait_state")});
    auto sub = hypha::Node().subscribe<Reading>(hypha::Topic{testTopic("/wait_state")});

    ASSERT_TRUE(pub.publish(Reading{42, "ready"}));
    (void)node.runFor(hypha::Duration::fromMillis(500));
    (void)node.runFor(hypha::Duration::fromMillis(1));

    const auto value = sub.latestSample();
    ASSERT_TRUE(value.has_value());
    EXPECT_EQ(value->message->value, 42);
}

TEST_F(NodeFixture, DispatchThreadDeliversWithoutManualPolling)
{
    hypha::Node node("node_test");

    std::atomic<int>        received{0};
    std::mutex              mutex;
    std::condition_variable condition;
    AsyncCounter            callback{&received, &mutex, &condition};
    auto        pub = hypha::Node().advertise<Reading>(hypha::Topic{testTopic("/async")});
    auto        sub = hypha::Node().subscribe<Reading>(hypha::Topic{testTopic("/async")}, callback);
    std::thread runnerThread([&node] {
        (void)node.run();
    });

    ASSERT_TRUE(pub.publish(Reading{7, "seven"}));
    std::unique_lock<std::mutex> lock(mutex);
    const bool arrived = condition.wait_for(lock, std::chrono::seconds(1), [&received] {
        return received.load(std::memory_order_acquire) == 1;
    });
    lock.unlock();
    hypha::requestStop();
    runnerThread.join();

    EXPECT_TRUE(arrived);
    EXPECT_EQ(received.load(std::memory_order_acquire), 1);
}

TEST_F(NodeFixture, DispatchThreadStopLeavesTheNodeUsable)
{
    hypha::Node node("node_test");

    std::atomic<int>        received{0};
    std::mutex              mutex;
    std::condition_variable condition;
    AsyncCounter            callback{&received, &mutex, &condition};
    auto pub = hypha::Node().advertise<Reading>(hypha::Topic{testTopic("/async_stop")});
    auto sub = hypha::Node().subscribe<Reading>(hypha::Topic{testTopic("/async_stop")}, callback);

    std::thread runnerThread([&node] {
        (void)node.run();
    });
    hypha::requestStop();
    runnerThread.join();

    // Stopping the worker must not stop the node. Foreground dispatch still
    // works, which is what separates requestStop() from releasing the node.
    EXPECT_TRUE(hypha::ok());

    ASSERT_TRUE(pub.publish(Reading{11, "eleven"}));
    for (int attempt = 0; attempt < 20; ++attempt) {
        (void)node.runFor(hypha::Duration::fromMillis(1));
        if (received.load(std::memory_order_acquire) == 1) {
            break;
        }
    }
    EXPECT_EQ(received.load(std::memory_order_acquire), 1);
}

TEST_F(NodeFixture, RawSampleTravelsToAMatchingPattern)
{
    hypha::Node node("node_test");

    // The sample's bytes are a view valid only inside the callback, so anything
    // kept afterwards must be copied here.
    std::string          topic;
    std::vector<uint8_t> payload;
    bool                 delivered = false;
    RawSampleCapture     callback{&topic, &payload, &delivered};

    // A pattern subscription resolves its topics once, when it is created,
    // so the publisher this test expects to reach must already exist.
    auto pub = hypha::rawPublisher(
        testTopic("/raw/topic"),
        {.dataType = "std_msgs/Bool",
         .md5 = "8b94c1b53db61fb6aed406028ad6332a",
         .definition = "bool data\n"}
    );
    ASSERT_TRUE(pub.has_value());
    EXPECT_EQ(pub->topic(), testTopic("/raw/topic"));

    auto sub = hypha::subscribeRaw(testTopic("/raw/**"), callback, 8).value();
    ASSERT_TRUE(sub.isValid());

    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(pub->publish({1}), hypha::Status::Ok);
    (void)node.runFor(hypha::Duration::fromMillis(1));

    EXPECT_TRUE(delivered);
    EXPECT_EQ(topic, testTopic("/raw/topic"));
    EXPECT_EQ(payload, std::vector<uint8_t>({1}));
}

TEST_F(NodeFixture, RawSubscribeMatchesASingleSegmentWildcard)
{
    hypha::Node node("node_test");

    bool delivered = false;

    // Both publishers exist before the pattern is resolved: a pattern
    // subscription follows the topics that exist when it is created.
    auto matching = hypha::rawPublisher(
                        testTopic("/raw/one/leaf"),
                        {.dataType = "std_msgs/Bool",
                         .md5 = "8b94c1b53db61fb6aed406028ad6332a",
                         .definition = "bool data\n"}
    )
                        .value();
    auto nonMatching = hypha::rawPublisher(
                           testTopic("/raw/one/two/leaf"),
                           {.dataType = "std_msgs/Bool",
                            .md5 = "8b94c1b53db61fb6aed406028ad6332a",
                            .definition = "bool data\n"}
    )
                           .value();

    auto sub = hypha::subscribeRaw(testTopic("/raw/*/leaf"), [&](const hypha::RawSample&) {
                   delivered = true;
               }).value();
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(matching.publish({1}), hypha::Status::Ok);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_TRUE(delivered);

    delivered = false;
    EXPECT_EQ(nonMatching.publish({1}), hypha::Status::Ok);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_FALSE(delivered);
}

TEST_F(NodeFixture, PlainMainStyleNodeWorksEndToEnd)
{
    hypha::Node node("demo_node");

    auto pub = hypha::Node().advertise<Reading>(hypha::Topic{testTopic("/demo")});

    int  received = 0;
    auto sub = hypha::Node().subscribe<Reading>(
        hypha::Topic{testTopic("/demo")}, [&](const hypha::MessagePtr<Reading>& msg) {
            received += msg->value;
        }
    );

    auto ticker = node.timer(100_ms, [&] {
        EXPECT_TRUE(pub.publish(Reading{1, "tick"}));
    });

    for (int i = 0; i < 5; ++i) {
        advance(100_ms);
        (void)node.runFor(hypha::Duration::fromMillis(1));
    }

    EXPECT_EQ(received, 5);
    EXPECT_TRUE(sub.latestSample().has_value());
}

TEST(NodeSetup, TimerWithoutANodePreservesStatus)
{
    hypha::Node       node;
    const std::string refusal = hypha_test::refusalOf([&node] {
        static_cast<void>(node.timer(100_ms, [] {
        }));
    });
    EXPECT_NE(refusal.find("InvalidState"), std::string::npos) << refusal;
}

/// A handle that joins no session reads no port table and opens no endpoint. The refusal
/// says that no session exists and what to do, not which table failed (#209).
TEST(NodeSetup, AnEndpointOfASessionlessHandleSaysThatNoSessionExists)
{
    const std::string publisher = hypha_test::refusalOf([] {
        static_cast<void>(hypha::Node().advertise<Reading>("/x"));
    });
    const std::string subscriber = hypha_test::refusalOf([] {
        static_cast<void>(hypha::Node().subscribe<Reading>("/x"));
    });
    const std::string literal = hypha_test::refusalOf([] {
        static_cast<void>(hypha::Node().advertise<Reading>(hypha::Topic{"/x"}));
    });
    for (const std::string& refusal : {publisher, subscriber, literal}) {
        EXPECT_NE(refusal.find("no session exists"), std::string::npos) << refusal;
        EXPECT_NE(refusal.find("create a named hypha::Node first"), std::string::npos) << refusal;
        EXPECT_NE(refusal.find("InvalidState"), std::string::npos) << refusal;
    }
    EXPECT_NE(publisher.find("ports.outputs./x"), std::string::npos) << publisher;
    EXPECT_NE(subscriber.find("ports.inputs./x"), std::string::npos) << subscriber;
    EXPECT_NE(literal.find("topic '/x'"), std::string::npos) << literal;
}

TEST(NodeSetup, RawSubscriberWithoutANodeIsRefused)
{
    const auto created = hypha::subscribeRaw("/no_node", [](const hypha::RawSample&) {
    });
    ASSERT_FALSE(created.has_value());
    EXPECT_EQ(created.error(), hypha::Status::InvalidState);
}

namespace {
struct BoundCallbacks
{
    int calls{0};

    int& increment(int amount)
    {
        this->calls += amount;
        return this->calls;
    }

    int read() const
    {
        return this->calls;
    }
};
}  // namespace

TEST(NodeSetup, MemberBindingBorrowsMutableAndConstObjectsAndPreservesReferences)
{
    BoundCallbacks object;
    auto           increment = hypha::bind<&BoundCallbacks::increment>(object);
    int&           count = increment(3);
    EXPECT_EQ(&count, &object.calls);
    const BoundCallbacks& readOnly = object;
    EXPECT_EQ(hypha::bind<&BoundCallbacks::read>(readOnly)(), 3);
}

TEST(NodeSetup, RequireReportsTheOperationAndOriginalStatus)
{
    try {
        hypha::require(
            std::expected<hypha::Timer, hypha::Status>(
                std::unexpected(hypha::Status::InvalidState)
            ),
            "status timer"
        );
        FAIL() << "the failed result was accepted";
    } catch (const std::runtime_error& error) {
        EXPECT_NE(std::string(error.what()).find("status timer"), std::string::npos);
        EXPECT_NE(std::string(error.what()).find("InvalidState"), std::string::npos)
            << error.what();
    }
}

namespace {

/// A timer callback that destroys its own timer on its first call.
struct DestroyOwnTimer
{
    std::optional<hypha::Timer>* slot;
    int*                         calls;

    void operator()() const
    {
        ++*this->calls;
        this->slot->reset();
    }
};

}  // namespace

TEST(TimerLifetime, ATimerDestroyedInItsOwnCallbackStopsAtOnceAndLogsNothing)
{
    hypha::Node                 node("timer_self_destroy");
    std::optional<hypha::Timer> timer;
    int                         calls = 0;
    timer.emplace(node.timer(hypha::Duration::fromMillis(5), DestroyOwnTimer{&timer, &calls}));

    ::testing::internal::CaptureStderr();
    ASSERT_TRUE(node.runFor(hypha::Duration::fromMillis(50)).ok());
    const std::string diagnostic = ::testing::internal::GetCapturedStderr();

    EXPECT_EQ(calls, 1);
    EXPECT_FALSE(timer.has_value());
    EXPECT_EQ(diagnostic.find("could not detach"), std::string::npos) << diagnostic;
}

TEST_F(NodeFixture, ACanceledTimerCallsNothingUntilAReset)
{
    hypha::Node node("timer_cancel");
    int         fired = 0;
    auto        timer = node.timer(100_ms, [&] {
        ++fired;
    });
    advance(150_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    ASSERT_EQ(fired, 1);

    ASSERT_TRUE(timer.cancel().has_value());
    ASSERT_TRUE(timer.cancel().has_value()) << "a cancel is idempotent";
    const auto canceled = timer.state();
    ASSERT_TRUE(canceled.has_value());
    EXPECT_TRUE(canceled->canceled);
    EXPECT_EQ(canceled->period, 100_ms);
    EXPECT_FALSE(canceled->untilDue.has_value());

    advance(300_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(fired, 1);
    EXPECT_EQ(timer.missedCount(), 0U) << "a canceled timer misses no period";

    ASSERT_TRUE(timer.reset().has_value());
    const auto reset = timer.state();
    ASSERT_TRUE(reset.has_value());
    EXPECT_FALSE(reset->canceled);
    EXPECT_EQ(reset->untilDue, std::optional<hypha::Duration>(100_ms));

    advance(50_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(fired, 1);
    advance(60_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(fired, 2);
    EXPECT_EQ(timer.missedCount(), 0U);
}

TEST_F(NodeFixture, AResetMovesTheNextCallOnePeriodPastTheReset)
{
    hypha::Node node("timer_reset");
    int         fired = 0;
    auto        timer = node.timer(100_ms, [&] {
        ++fired;
    });
    advance(80_ms);
    ASSERT_TRUE(timer.reset().has_value());

    // The call the timer had due at 100 ms now comes at 180 ms.
    advance(50_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(fired, 0);
    const auto state = timer.state();
    ASSERT_TRUE(state.has_value());
    EXPECT_EQ(state->untilDue, std::optional<hypha::Duration>(50_ms));

    advance(60_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(fired, 1);
}

TEST_F(NodeFixture, ACancelFromACallbackHoldsBackACallDueLaterInTheSameTurn)
{
    hypha::Node                 node("timer_cancel_other");
    std::optional<hypha::Timer> second;
    int                         firstCalls = 0;
    int                         secondCalls = 0;
    auto                        first = node.timer(100_ms, [&] {
        ++firstCalls;
        ASSERT_TRUE(second->cancel().has_value());
    });
    second.emplace(node.timer(100_ms, [&] {
        ++secondCalls;
    }));

    // Both are due in this turn; the first, attached first, runs first.
    advance(150_ms);
    (void)node.runFor(hypha::Duration::fromMillis(1));
    EXPECT_EQ(firstCalls, 1);
    EXPECT_EQ(secondCalls, 0);
}

TEST(TimerControl, AnInvalidTimerIsRefused)
{
    hypha::Timer timer;
    EXPECT_EQ(timer.cancel().error(), hypha::Status::InvalidState);
    EXPECT_EQ(timer.reset().error(), hypha::Status::InvalidState);
    EXPECT_EQ(timer.state().error(), hypha::Status::InvalidState);
}

TEST(TimerControl, ATimerCreatedInACallbackOnHostTimeFiresAndIsControlled)
{
    hypha::Node                 node("timer_in_callback");
    std::optional<hypha::Timer> late;
    int                         lateCalls = 0;
    auto                        starter = node.timer(hypha::Duration::fromMillis(1), [&] {
        if (late.has_value()) {
            return;
        }
        late.emplace(node.timer(hypha::Duration::fromMillis(2), [&] {
            ++lateCalls;
            ASSERT_TRUE(late->cancel().has_value());
        }));
        const auto state = late->state();
        ASSERT_TRUE(state.has_value());
        EXPECT_FALSE(state->canceled);
        ASSERT_TRUE(state->untilDue.has_value());
        EXPECT_LE(*state->untilDue, hypha::Duration::fromMillis(2));
    });

    // Only outcomes: the late timer fired once and canceled itself.
    for (int slice = 0; slice < 200 && lateCalls == 0; ++slice) {
        ASSERT_TRUE(node.runFor(hypha::Duration::fromMillis(10)).ok());
    }
    ASSERT_TRUE(late.has_value());
    EXPECT_EQ(lateCalls, 1);
    const auto state = late->state();
    ASSERT_TRUE(state.has_value());
    EXPECT_TRUE(state->canceled);
}
