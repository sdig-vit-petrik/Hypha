// The cycle examples of the documentation build as written: a missing overload or a
// renamed call fails the build. The tests run the step tests of cpp-api section 5.4.

#include "hypha/cycle_node.h"
#include "hypha/cycle_transforms.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"
#include <hypha_test_srvs/String.h>
#include <hypha_test_srvs/Vec1.h>

#include <gtest/gtest.h>
#include <std_msgs/Float64.h>
#include <std_srvs/SetBool.h>
#include <std_srvs/Trigger.h>

#include <deque>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

// cpp-api 10.1: a TF lookup in a step.
class Planner final : public hypha::CycleNode
{
  public:
    Planner()
    {
        this->setRate(hypha::Hz(50.0));
    }

    hypha::Transforms tf{*this};  // 10 s of history per frame
    double            baseX{0.0};

    void step(const hypha::CycleTime&) override
    {
        // The latest time common to the chain.
        const auto found = this->tf.lookupLatest("map", "base_link");
        if (found) {
            this->baseX = found->transform.translation.x;  // base_link in map
        }
    }
};

// Spec 4.4 and cpp-api 5.5: the level 2 cycle with a deque of ports.
struct Relay
{
    std::deque<hypha::Latest<std_msgs::Float64>>* inputs;
    hypha::Output<std_msgs::Float64>*             sum;

    void operator()(const hypha::CycleTime&) const
    {
        double total = 0.0;
        for (const auto& input : *this->inputs) {
            if (input.usable()) {
                total += input->data;
            }
        }
        this->sum->send({.data = total});
    }
};

[[maybe_unused]] void buildRelay(hypha::Node& node, const std::vector<std::string>& topicParameters)
{
    hypha::Cycle                                 cycle("relay");
    std::deque<hypha::Latest<std_msgs::Float64>> inputs;  // deque: elements never move
    for (const std::string& parameter : topicParameters) {
        inputs.emplace_back(cycle, parameter);
    }
    hypha::Output<std_msgs::Float64> sum{cycle, "topic_sum"};
    cycle.setRate(hypha::Hz(50.0));
    hypha::require(cycle.start(node, Relay{&inputs, &sum}));
}

// cpp-api 5.4: the level 3 node class.
class Controller final : public hypha::CycleNode
{
  public:
    hypha::Latest<std_msgs::Float64>   speed{*this, "topic_speed"};
    hypha::Arrivals<std_msgs::Float64> setpoints{*this, "topic_setpoint"};
    hypha::Output<std_msgs::Float64>   command{*this, "topic_command"};

    void setup(hypha::Node&) override
    {
        this->setRate(hypha::Hz(100.0));
    }

    void step(const hypha::CycleTime&) override
    {
        if (!this->speed.usable()) {
            return;
        }
        this->command.send({.data = -this->speed->data});
    }
};

// cpp-api: a step tested without a bus. The harness sets what a capture would set and
// keeps what the step sends.
TEST(StepWithoutABus, ASetInputReachesTheStepAndTheSentMessageIsReadable)
{
    Controller                  controller;
    hypha::testing::StepHarness harness{controller};
    harness.set(controller.speed, std_msgs::Float64{4.0});
    hypha::CycleTime time;
    time.now = hypha::Time::fromNanos(10'000'000);
    harness.step(controller, time);
    ASSERT_TRUE(controller.command.sent());
    ASSERT_EQ(harness.sent(controller.command).size(), 1U);
    EXPECT_DOUBLE_EQ(harness.sent(controller.command).front().data, -4.0);
}

TEST(StepWithoutABus, AnOutputThatSentNothingHasNoMessage)
{
    Controller                  controller;
    hypha::testing::StepHarness harness{controller};
    harness.setStale(controller.speed);
    harness.step(controller);
    EXPECT_FALSE(controller.command.sent());
    EXPECT_TRUE(harness.sent(controller.command).empty());
}

TEST(StepWithoutABus, InputsOkFollowsTheSetVerdictAndFaults)
{
    Controller                  controller;
    hypha::testing::StepHarness harness{controller};
    harness.set(controller.speed, std_msgs::Float64{1.0});
    harness.set(controller.setpoints, {});
    EXPECT_TRUE(controller.inputsOk());

    harness.setStagnant(controller.speed, true);
    EXPECT_TRUE(controller.speed.stagnant());
    EXPECT_FALSE(controller.speed.stale());
    EXPECT_FALSE(controller.inputsOk()) << "a stagnant input is not ok";

    harness.setStagnant(controller.speed, false);
    harness.setStale(controller.speed);
    EXPECT_TRUE(controller.speed.stale());
    EXPECT_FALSE(controller.inputsOk()) << "a stale input is not ok";

    harness.setNeverArrived(controller.speed);
    EXPECT_FALSE(controller.inputsOk()) << "an input that never arrived is not ok";
}

// cpp-api 8.6: a chain of calls. hypha_test_srvs has the fields of mrs_msgs/String and
// mrs_msgs/Vec1.
namespace mrs_msgs {
using String = hypha_test_srvs::String;
using Vec1 = hypha_test_srvs::Vec1;
}  // namespace mrs_msgs

class UavManager final : public hypha::CycleNode
{
  public:
    Requests<std_srvs::Trigger> takeoff{*this, "takeoff"};
    Calls<mrs_msgs::String>     switchController{*this, "switch_controller"};
    Calls<mrs_msgs::String>     switchTracker{*this, "switch_tracker"};
    Calls<mrs_msgs::Vec1>       trackerTakeoff{*this, "tracker_takeoff"};
    Calls<std_srvs::SetBool>    toggleOutput{*this, "toggle_output"};
    Calls<std_srvs::SetBool>    arming{*this, "arming"};
    Calls<std_srvs::Trigger>    eland{*this, "eland"};

    void step(const hypha::CycleTime& time) override
    {
        for (hypha::Request<std_srvs::Trigger>& request : this->takeoff.requests()) {
            if (this->takeoffSeq_.running()) {
                request.reply({.success = false, .message = "takeoff in progress"});
            } else {
                this->takeoffSeq_.start(request.defer(), this->runTakeoff());
            }
        }
        this->takeoffSeq_.advance(time);  // in every step while it runs
    }

  private:
    hypha::Task<std_srvs::Trigger::Response> runTakeoff()
    {
        auto a = co_await hypha::call(this->switchController, {.value = "MpcController"});
        if (!succeeded(a)) {
            co_return co_await this->disarm("could not activate the controller", a);
        }
        auto b = co_await hypha::call(this->switchTracker, {.value = "LandoffTracker"});
        if (!succeeded(b)) {
            co_return co_await this->disarm("could not activate the tracker", b);
        }
        co_await hypha::sleepFor(hypha::Duration::fromMillis(300));  // the motors ramp up
        auto c = co_await hypha::call(this->trackerTakeoff, {.goal = this->takeoffHeight_});
        if (!succeeded(c)) {
            auto e = co_await hypha::call(this->eland, {});
            co_return {
                .success = false,
                .message = "takeoff was not successful (" + c.description()
                         + "); eland: " + e.description()
            };
        }
        co_return {.success = true, .message = "taking off"};
    }

    /// Turn the output off, then disarm, and fail the takeoff with the reason.
    template <typename Srv>
    hypha::Task<std_srvs::Trigger::Response> disarm(std::string what, hypha::CallResult<Srv> why)
    {
        co_await hypha::call(this->toggleOutput, {.data = false});
        co_await hypha::call(this->arming, {.data = false});
        co_return {.success = false, .message = what + " (" + why.description() + ")"};
    }

    template <typename Srv>
    static bool succeeded(const hypha::CallResult<Srv>& r)
    {
        return r.ok() && r.response().success;
    }

    hypha::Sequence<std_srvs::Trigger> takeoffSeq_;
    double                             takeoffHeight_{1.5};
};

}  // namespace
