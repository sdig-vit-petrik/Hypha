// hypha::Sequence: a chain of calls in the steps of a cycle that ends in the answer to a
// deferred request (docs/design/native-services.md §8.2), driven by testing::StepHarness.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include "hypha/sequence.h"
#include "hypha/testing.h"
#include <hypha_test_srvs/String.h>
#include <hypha_test_srvs/Vec1.h>

#include <gtest/gtest.h>
#include <std_msgs/Bool.h>
#include <std_srvs/SetBool.h>
#include <std_srvs/Trigger.h>

#include <memory>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace {

using hypha::CallOutcome;
using hypha::Time;
using SetBool = std_srvs::SetBool;
using Trigger = std_srvs::Trigger;
using Name = hypha_test_srvs::String;
using Goal = hypha_test_srvs::Vec1;

hypha::CycleTime at(int64_t millis)
{
    hypha::CycleTime time;
    time.now = Time::fromNanos(millis * 1'000'000);
    return time;
}

/// Three calls in a row; the request is answered with what became of them.
class Chain final : public hypha::CycleNode
{
  public:
    Requests<Trigger> start{*this, "start"};
    Calls<SetBool>    first{*this, "first"};
    Calls<SetBool>    second{*this, "second"};
    Calls<SetBool>    third{*this, "third"};
    bool              advances{true};

    void step(const hypha::CycleTime& time) override
    {
        for (hypha::Request<Trigger>& request : this->start.requests()) {
            if (this->chain.running()) {
                static_cast<void>(request.reply({.success = false, .message = "busy"}));
            } else {
                this->chain.start(request.defer(), this->run());
            }
        }
        if (this->advances) {
            this->chain.advance(time);
        }
    }

    hypha::Sequence<Trigger> chain;

  private:
    hypha::Task<Trigger::Response> run()
    {
        for (Calls<SetBool>* port : {&this->first, &this->second, &this->third}) {
            const hypha::CallResult<SetBool> result = co_await hypha::call(*port, {.data = true});
            if (!result.ok()) {
                co_return {.success = false, .message = port->name() + ": " + result.description()};
            }
        }
        co_return {.success = true, .message = "all three"};
    }
};

TEST(Sequence, ThreeCallsInARowAnswerTheRequest)
{
    Chain                       node;
    hypha::testing::StepHarness harness{node};
    auto                        request = harness.request(node.start, {});
    harness.step(node, at(0));
    EXPECT_TRUE(request.deferred());
    EXPECT_EQ(harness.calls(node.first).size(), 1U);
    EXPECT_EQ(node.chain.waitingFor(), "the outcome of a call to 'first'");
    harness.answer(node.first, {.success = true});
    harness.step(node, at(10));
    EXPECT_EQ(harness.calls(node.second).size(), 1U);
    harness.answer(node.second, {.success = true});
    harness.step(node, at(20));
    harness.answer(node.third, {.success = true});
    harness.step(node, at(30));
    ASSERT_TRUE(request.replied()) << request.describe();
    EXPECT_EQ(request.response().message, "all three");
    EXPECT_EQ(request.answeredAt(), Time::fromNanos(30'000'000));
    EXPECT_FALSE(node.chain.running());
    EXPECT_EQ(node.chain.lastAnswer(), hypha::Status::Ok);
}

TEST(Sequence, AFailureInTheMiddleAnswersTheRequestWithTheReason)
{
    Chain                       node;
    hypha::testing::StepHarness harness{node};
    auto                        request = harness.request(node.start, {});
    harness.step(node, at(0));
    harness.answer(node.first, {.success = true});
    harness.step(node, at(10));
    harness.outcome(node.second, CallOutcome::Refused, "not armed");
    harness.step(node, at(20));
    ASSERT_TRUE(request.replied()) << request.describe();
    EXPECT_FALSE(request.response().success);
    EXPECT_EQ(request.response().message, "second: refused: not armed");
    EXPECT_TRUE(harness.calls(node.third).empty()) << "the chain stopped at the failure";
    EXPECT_FALSE(node.chain.running());
}

TEST(Sequence, ARequestWhileItRunsIsAnsweredByTheStep)
{
    Chain                       node;
    hypha::testing::StepHarness harness{node};
    auto                        first = harness.request(node.start, {});
    harness.step(node, at(0));
    auto second = harness.request(node.start, {});
    harness.step(node, at(10));
    EXPECT_TRUE(first.deferred());
    ASSERT_TRUE(second.replied());
    EXPECT_EQ(second.response().message, "busy");
}

TEST(Sequence, AStepThatDoesNotAdvanceDropsTheOutcomeAndTheNextAdvanceSaysSo)
{
    Chain                       node;
    hypha::testing::StepHarness harness{node};
    auto                        request = harness.request(node.start, {});
    harness.step(node, at(0));
    harness.answer(node.first, {.success = true});
    node.advances = false;
    harness.step(node, at(10));
    node.advances = true;
    try {
        harness.step(node, at(20));
        FAIL() << "the advance after a dropped outcome throws";
    } catch (const std::logic_error& error) {
        EXPECT_NE(std::string(error.what()).find("'first'"), std::string::npos) << error.what();
    }
    EXPECT_FALSE(node.chain.running());
    EXPECT_TRUE(request.failed()) << "a failed step answers the request as failed: "
                                  << request.describe();
}

/// One call, answered with its outcome.
hypha::Task<Trigger::Response> callOnce(hypha::Calls<SetBool>& port)
{
    const hypha::CallResult<SetBool> result = co_await hypha::call(port, {.data = true});
    co_return {.success = result.ok(), .message = result.description()};
}

TEST(Sequence, DestroyingARunningSequenceRefusesItsRequest)
{
    Chain                       node;
    hypha::testing::StepHarness harness{node};
    auto                        sequence = std::make_unique<hypha::Sequence<Trigger>>();
    auto                        request = harness.request(node.start, {});
    harness.step(
        [&node, &sequence](const hypha::CycleTime& time) {
            for (hypha::Request<Trigger>& r : node.start.requests()) {
                sequence->start(r.defer(), callOnce(node.first));
            }
            sequence->advance(time);
        },
        at(0)
    );
    EXPECT_TRUE(request.deferred());
    EXPECT_THROW(sequence->start({}, callOnce(node.first)), std::logic_error);
    sequence.reset();
    ASSERT_TRUE(request.refused()) << request.describe();
}

/// A Task that throws, and a call the port refuses at once.
class Faulty final : public hypha::CycleNode
{
  public:
    Requests<Trigger> start{*this, "start"};
    Calls<SetBool>    port{*this, "port"};
    bool              busyFirst{false};
    std::string       seen;

    void step(const hypha::CycleTime& time) override
    {
        for (hypha::Request<Trigger>& request : this->start.requests()) {
            this->chain.start(request.defer(), this->run());
        }
        this->chain.advance(time);
    }

    hypha::Sequence<Trigger> chain;

  private:
    hypha::Task<Trigger::Response> run()
    {
        if (this->busyFirst) {
            static_cast<void>(this->port.call({.data = true}));  // takes the one slot
            const hypha::CallResult<SetBool> busy = co_await hypha::call(this->port, {});
            this->seen = busy.description();
            co_return {.success = busy.ok(), .message = busy.description()};
        }
        const hypha::CallResult<SetBool> result = co_await hypha::call(this->port, {});
        if (!result.ok()) {
            throw std::runtime_error("no answer: " + result.description());
        }
        co_return {.success = true};
    }
};

TEST(Sequence, ATaskThatThrowsFailsTheStepAndItsRequest)
{
    Faulty                      node;
    hypha::testing::StepHarness harness{node};
    auto                        request = harness.request(node.start, {});
    harness.step(node, at(0));
    harness.outcome(node.port, CallOutcome::ServerLost, "gone");
    EXPECT_THROW(harness.step(node, at(10)), std::runtime_error);
    EXPECT_TRUE(request.failed()) << request.describe();
    EXPECT_FALSE(node.chain.running());
}

TEST(Sequence, ABusyCallResumesAtOnceAsNotSent)
{
    Faulty                      node;
    hypha::testing::StepHarness harness{node};
    node.busyFirst = true;
    auto request = harness.request(node.start, {});
    harness.step(node, at(0));
    EXPECT_EQ(node.seen, std::string("not sent: ") + hypha::toString(hypha::CallStatus::Busy));
    ASSERT_TRUE(request.replied()) << request.describe();
    EXPECT_FALSE(request.response().success);
}

/// The shape of MRS uav_manager's takeoff: switch the controller, then the tracker, let the
/// motors ramp up, take off, answer; then wait for the tracker to have its goal and to
/// reach it, and switch to the trackers for flight.
class UavManager final : public hypha::CycleNode
{
  public:
    Requests<Trigger>        takeoff{*this, "takeoff"};
    Calls<Name>              switchController{*this, "switch_controller"};
    Calls<Name>              switchTracker{*this, "switch_tracker"};
    Calls<Goal>              trackerTakeoff{*this, "tracker_takeoff"};
    Calls<SetBool>           toggleOutput{*this, "toggle_output"};
    Calls<SetBool>           arming{*this, "arming"};
    Calls<Trigger>           eland{*this, "eland"};
    Latest<std_msgs::Bool>   haveGoal{*this, "have_goal"};
    std::vector<std::string> log;

    void step(const hypha::CycleTime& time) override
    {
        for (hypha::Request<Trigger>& request : this->takeoff.requests()) {
            if (this->takeoffSeq.running() || this->afterSeq.running()) {
                static_cast<void>(
                    request.reply({.success = false, .message = "takeoff in progress"})
                );
            } else {
                this->takeoffSeq.start(request.defer(), this->runTakeoff());
            }
        }
        this->takeoffSeq.advance(time);
        this->afterSeq.advance(time);
    }

    hypha::Sequence<Trigger> takeoffSeq;
    hypha::Sequence<>        afterSeq;

  private:
    hypha::Task<Trigger::Response> runTakeoff()
    {
        auto a = co_await hypha::call(this->switchController, {.value = "mpc"});
        if (!succeeded(a)) {
            co_return co_await this->disarm(
                "could not activate 'mpc' for takeoff", a.description()
            );
        }
        auto b = co_await hypha::call(this->switchTracker, {.value = "landoff"});
        if (!succeeded(b)) {
            co_return co_await this->disarm(
                "could not activate 'landoff' for takeoff", b.description()
            );
        }
        co_await hypha::sleepFor(hypha::Duration::fromMillis(300));  // the motors ramp up
        auto c = co_await hypha::call(this->trackerTakeoff, {.goal = 1.5});
        if (!succeeded(c)) {
            auto e = co_await hypha::call(this->eland, {});
            this->log.push_back("eland: " + e.description());
            co_return {
                .success = false, .message = "takeoff was not successful (" + c.description() + ")"
            };
        }
        this->afterSeq.start(this->afterTakeoff());
        co_return {.success = true, .message = "taking off"};
    }

    hypha::Task<> afterTakeoff()
    {
        co_await hypha::until([this] {
            return this->trackerHas(true);
        });
        co_await hypha::until([this] {
            return this->trackerHas(false);
        });
        auto r = co_await hypha::call(this->switchTracker, {.value = "mpc_tracker"});
        this->log.push_back("after: " + r.description());
    }

    /// Turn the output off and disarm, one call after the other, then fail the takeoff.
    hypha::Task<Trigger::Response> disarm(std::string what, std::string why)
    {
        auto off = co_await hypha::call(this->toggleOutput, {.data = false});
        auto disarmed = co_await hypha::call(this->arming, {.data = false});
        this->log.push_back("disarm: " + off.description() + ", " + disarmed.description());
        co_return {.success = false, .message = what + " (" + why + ")"};
    }

    bool trackerHas(bool goal) const
    {
        return this->haveGoal.usable() && this->haveGoal->data == goal;
    }

    template <typename Srv>
    static bool succeeded(const hypha::CallResult<Srv>& r)
    {
        return r.ok() && r.response().success;
    }
};

TEST(Sequence, TheTakeoffRunsInOrderAndAnswersBeforeTheWorkAfterIt)
{
    UavManager                  node;
    hypha::testing::StepHarness harness{node};
    harness.set(node.haveGoal, std_msgs::Bool{});
    auto request = harness.request(node.takeoff, {});
    harness.step(node, at(0));
    ASSERT_EQ(harness.calls(node.switchController).size(), 1U);
    EXPECT_EQ(harness.calls(node.switchController)[0].value, "mpc");
    harness.answer(node.switchController, {.success = true});
    harness.step(node, at(10));
    ASSERT_EQ(harness.calls(node.switchTracker).size(), 1U);
    harness.answer(node.switchTracker, {.success = true});
    harness.step(node, at(20));  // the sleep begins
    EXPECT_EQ(node.takeoffSeq.waitingFor(), "the end of a sleep");
    harness.step(node, at(310));
    EXPECT_TRUE(harness.calls(node.trackerTakeoff).empty()) << "300 ms are not over";
    harness.step(node, at(320));
    ASSERT_EQ(harness.calls(node.trackerTakeoff).size(), 1U);
    EXPECT_EQ(harness.calls(node.trackerTakeoff)[0].goal, 1.5);
    harness.answer(node.trackerTakeoff, {.success = true});
    auto again = harness.request(node.takeoff, {});
    harness.step(node, at(330));
    ASSERT_TRUE(request.replied()) << request.describe();
    EXPECT_TRUE(request.response().success);
    EXPECT_EQ(again.response().message, "takeoff in progress");
    EXPECT_TRUE(node.afterSeq.running());
    harness.set(node.haveGoal, std_msgs::Bool{.data = true});
    harness.step(node, at(340));
    EXPECT_EQ(node.afterSeq.waitingFor(), "a condition");
    harness.set(node.haveGoal, std_msgs::Bool{.data = false});
    harness.step(node, at(350));
    ASSERT_EQ(harness.calls(node.switchTracker).size(), 1U);
    EXPECT_EQ(harness.calls(node.switchTracker)[0].value, "mpc_tracker");
    harness.answer(node.switchTracker, {.success = true});
    harness.step(node, at(360));
    EXPECT_FALSE(node.afterSeq.running());
    EXPECT_EQ(node.log, std::vector<std::string>{"after: answered"});
}

TEST(Sequence, ATrackerThatDoesNotSwitchDisarmsAndRefusesTheTakeoff)
{
    UavManager                  node;
    hypha::testing::StepHarness harness{node};
    auto                        request = harness.request(node.takeoff, {});
    harness.step(node, at(0));
    harness.answer(node.switchController, {.success = true});
    harness.step(node, at(10));
    harness.outcome(node.switchTracker, CallOutcome::Timeout, "after 1 s");
    harness.step(node, at(20));
    EXPECT_EQ(harness.calls(node.toggleOutput).size(), 1U);
    EXPECT_TRUE(harness.calls(node.arming).empty()) << "one call after the other";
    harness.answer(node.toggleOutput, {.success = true});
    harness.step(node, at(30));
    ASSERT_EQ(harness.calls(node.arming).size(), 1U);
    EXPECT_FALSE(harness.calls(node.arming)[0].data);
    harness.outcome(node.arming, CallOutcome::NoServer, "no live server");
    harness.step(node, at(40));
    ASSERT_TRUE(request.replied()) << request.describe();
    EXPECT_FALSE(request.response().success);
    EXPECT_EQ(
        request.response().message, "could not activate 'landoff' for takeoff (timeout after 1 s)"
    );
    EXPECT_EQ(node.log, std::vector<std::string>{"disarm: answered, no server: no live server"});
    EXPECT_TRUE(harness.calls(node.trackerTakeoff).empty());
}

TEST(Sequence, AFailedTakeoffCallsEland)
{
    UavManager                  node;
    hypha::testing::StepHarness harness{node};
    auto                        request = harness.request(node.takeoff, {});
    harness.step(node, at(0));
    harness.answer(node.switchController, {.success = true});
    harness.step(node, at(10));
    harness.answer(node.switchTracker, {.success = true});
    harness.step(node, at(20));
    harness.step(node, at(320));
    harness.answer(node.trackerTakeoff, {.success = false, .message = "no odometry"});
    harness.step(node, at(330));
    ASSERT_EQ(harness.calls(node.eland).size(), 1U);
    harness.answer(node.eland, {.success = true});
    harness.step(node, at(340));
    ASSERT_TRUE(request.replied()) << request.describe();
    EXPECT_EQ(request.response().message, "takeoff was not successful (answered)");
    EXPECT_FALSE(node.afterSeq.running());
}

/// What a run of the takeoff called, and when: the same inputs give the same run.
std::vector<std::string> takeoffRun()
{
    UavManager                  node;
    hypha::testing::StepHarness harness{node};
    std::vector<std::string>    seen;
    auto                        request = harness.request(node.takeoff, {});
    for (int64_t millis = 0; millis <= 400; millis += 10) {
        harness.step(node, at(millis));
        const auto answerIfCalled = [&](auto& port) {
            if (!harness.calls(port).empty()) {
                seen.push_back(std::to_string(millis) + " " + port.name());
                harness.answer(port, {.success = true});
            }
        };
        answerIfCalled(node.switchController);
        answerIfCalled(node.switchTracker);
        answerIfCalled(node.trackerTakeoff);
    }
    seen.push_back(request.describe() + " at " + std::to_string(request.answeredAt().nanos()));
    return seen;
}

TEST(Sequence, TheSameInputsGiveTheSameRun)
{
    const std::vector<std::string> first = takeoffRun();
    EXPECT_EQ(first.size(), 4U);
    EXPECT_EQ(first.front(), "0 switch_controller");
    EXPECT_EQ(first, takeoffRun());
}

}  // namespace
