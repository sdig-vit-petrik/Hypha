// The service ports in a testing::StepHarness: the test is the client of a Requests port
// and the server of a Calls port, and the library decides what a step reads, answers,
// sends and carries, as for a started cycle on simulated time (docs/cpp-api.md §9).

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"

#include <gtest/gtest.h>
#include <std_srvs/SetBool.h>

#include <filesystem>
#include <fstream>
#include <functional>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

using SetBool = std_srvs::SetBool;
using hypha::CallOutcome;
using hypha::Time;
using hypha::testing::RequestState;

hypha::CycleTime at(int64_t millis)
{
    hypha::CycleTime time;
    time.now = Time::fromNanos(millis * 1'000'000);
    return time;
}

Time ms(int64_t millis)
{
    return Time::fromNanos(millis * 1'000'000);
}

/// A server whose step does what `serve` says with each request it reads.
class Switch final : public hypha::CycleNode
{
  public:
    Requests<SetBool>                             enable{*this, "enable"};
    bool                                          reads{true};
    bool                                          throws{false};
    std::function<void(hypha::Request<SetBool>&)> serve = [](hypha::Request<SetBool>& r) {
        static_cast<void>(r.reply({.success = r->data, .message = "from " + r.clientNode()}));
    };
    std::vector<std::string> order;
    hypha::Deferred<SetBool> held;

    void step(const hypha::CycleTime&) override
    {
        if (!this->reads) {
            return;
        }
        for (hypha::Request<SetBool>& request : this->enable.requests()) {
            this->order.push_back(request.clientNode());
            this->serve(request);
        }
        if (this->throws) {
            throw std::runtime_error("step failed");
        }
    }
};

/// A client whose step calls when `callNow` and reads what `read` says.
class Arming final : public hypha::CycleNode
{
  public:
    Calls<SetBool>                          arming{*this, "arming"};
    bool                                    callNow{false};
    int                                     callsPerStep{1};
    bool                                    read{true};
    bool                                    throws{false};
    std::vector<hypha::CallStatus>          statuses;
    std::vector<hypha::CallResult<SetBool>> results;
    uint64_t                                unread{0};

    void step(const hypha::CycleTime&) override
    {
        if (this->read) {
            for (hypha::CallResult<SetBool>& result : this->arming.results()) {
                this->results.push_back(std::move(result));
            }
        }
        this->unread = this->arming.unreadResults();
        for (int call = 0; this->callNow && call < this->callsPerStep; ++call) {
            this->statuses.push_back(this->arming.call({.data = true}));
        }
        this->callNow = false;
        if (this->throws) {
            throw std::runtime_error("step failed");
        }
    }
};

TEST(StepHarnessServices, AStepRepliesAndTheAnswerLeavesAfterIt)
{
    Switch                      node;
    hypha::testing::StepHarness harness{node};
    auto probe = harness.request(node.enable, {.data = true}, {.client = "mission"});
    EXPECT_TRUE(probe.queued());
    harness.step(node, at(10));
    ASSERT_TRUE(probe.replied()) << probe.describe();
    EXPECT_TRUE(probe.response().success);
    EXPECT_EQ(probe.response().message, "from mission");
    EXPECT_EQ(probe.answeredAt(), ms(10));
}

TEST(StepHarnessServices, AStepReadsTheRequestsSentBeforeItsTime)
{
    Switch                      node;
    hypha::testing::StepHarness harness{node};
    auto early = harness.request(node.enable, {.data = true}, {.sentAt = ms(5)});
    auto now = harness.request(node.enable, {.data = true}, {.sentAt = ms(10)});
    harness.step(node, at(10));
    EXPECT_TRUE(early.replied()) << early.describe();
    EXPECT_TRUE(now.queued()) << "a request sent at the step's time waits for a later step";
    harness.step(node, at(20));
    EXPECT_TRUE(now.replied()) << now.describe();
    EXPECT_EQ(now.answeredAt(), ms(20));
}

TEST(StepHarnessServices, RequestsAreReadInOrderOfSendTimeThenClient)
{
    Switch                      node;
    hypha::testing::StepHarness harness{node};
    static_cast<void>(harness.request(node.enable, {}, {.client = "c", .sentAt = ms(2)}));
    static_cast<void>(harness.request(node.enable, {}, {.client = "b", .sentAt = ms(1)}));
    static_cast<void>(harness.request(node.enable, {}, {.client = "a", .sentAt = ms(2)}));
    harness.step(node, at(10));
    EXPECT_EQ(node.order, (std::vector<std::string>{"b", "a", "c"}));
}

TEST(StepHarnessServices, AStepThatDoesNotReadLeavesItsRequestsQueued)
{
    Switch                      node;
    hypha::testing::StepHarness harness{node};
    node.reads = false;
    auto probe = harness.request(node.enable, {.data = true});
    harness.step(node, at(10));
    EXPECT_TRUE(probe.queued()) << probe.describe();
    node.reads = true;
    harness.step(node, at(20));
    EXPECT_TRUE(probe.replied()) << probe.describe();
}

TEST(StepHarnessServices, ARefusalAndARequestLeftAloneAreRefusedAfterTheStep)
{
    Switch                      node;
    hypha::testing::StepHarness harness{node};
    node.serve = [](hypha::Request<SetBool>& request) {
        if (!request->data) {
            static_cast<void>(request.refuse("not armed"));
        }
    };
    auto refused = harness.request(node.enable, {.data = false});
    auto left = harness.request(node.enable, {.data = true});
    harness.step(node, at(10));
    EXPECT_EQ(refused.state(), RequestState::Refused);
    EXPECT_EQ(refused.reason(), "not armed");
    ASSERT_TRUE(left.refused()) << left.describe();
    EXPECT_NE(left.reason().find("did not answer"), std::string::npos) << left.reason();
    EXPECT_THROW(static_cast<void>(left.response()), std::logic_error);
}

TEST(StepHarnessServices, ADeferredRequestIsAnsweredInALaterStep)
{
    Switch                      node;
    hypha::testing::StepHarness harness{node};
    node.serve = [&node](hypha::Request<SetBool>& request) {
        node.held = request.defer();
    };
    auto probe = harness.request(node.enable, {.data = true});
    harness.step(node, at(10));
    EXPECT_TRUE(probe.deferred()) << probe.describe();
    harness.step(
        [&node](const hypha::CycleTime&) {
            static_cast<void>(node.held.reply({.success = true, .message = "later"}));
        },
        at(20)
    );
    ASSERT_TRUE(probe.replied()) << probe.describe();
    EXPECT_EQ(probe.response().message, "later");
    EXPECT_EQ(probe.answeredAt(), ms(20));
}

TEST(StepHarnessServices, AFailedStepAnswersWhatItReadAsFailed)
{
    Switch                      node;
    hypha::testing::StepHarness harness{node};
    node.throws = true;
    auto probe = harness.request(node.enable, {.data = true});
    EXPECT_THROW(harness.step(node, at(10)), std::runtime_error);
    ASSERT_TRUE(probe.failed()) << probe.describe();
    EXPECT_EQ(probe.reason(), "server step failed");
}

TEST(StepHarnessServices, ADeferredAnswerOfAFailedStepIsFailed)
{
    Switch                      node;
    hypha::testing::StepHarness harness{node};
    node.serve = [&node](hypha::Request<SetBool>& request) {
        node.held = request.defer();
    };
    auto probe = harness.request(node.enable, {.data = true});
    harness.step(node, at(10));
    EXPECT_THROW(
        harness.step(
            [&node](const hypha::CycleTime&) {
                static_cast<void>(node.held.reply({.success = true}));
                throw std::runtime_error("after the answer");
            },
            at(20)
        ),
        std::runtime_error
    );
    EXPECT_TRUE(probe.failed()) << probe.describe();
}

TEST(StepHarnessServices, DestroyingADeferredRequestRefusesIt)
{
    Switch                      node;
    hypha::testing::StepHarness harness{node};
    node.serve = [&node](hypha::Request<SetBool>& request) {
        node.held = request.defer();
    };
    auto probe = harness.request(node.enable, {.data = true});
    harness.step(node, at(10));
    node.held = {};
    ASSERT_TRUE(probe.refused()) << probe.describe();
    EXPECT_NE(probe.reason().find("destroyed unanswered"), std::string::npos) << probe.reason();
}

/// A parameter file with the tables of the service ports.
class ServiceTables : public ::testing::Test
{
  protected:
    void load(const std::string& text)
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_step_harness_services.toml";
        std::ofstream(this->path) << text;
        this->context = std::make_unique<hypha::ConfigContext>(this->path.string());
    }

    void TearDown() override
    {
        this->context.reset();
        std::filesystem::remove(this->path);
    }

    std::unique_ptr<hypha::ConfigContext> context;
    std::filesystem::path                 path;
};

TEST_F(ServiceTables, PerStepBoundsTheRequestsAStepReads)
{
    this->load("[ports.servers.enable]\nservice = \"~/enable\"\nper_step = 2\n");
    Switch                      node;
    hypha::testing::StepHarness harness{node};
    auto first = harness.request(node.enable, {.data = true}, {.sentAt = ms(1)});
    auto second = harness.request(node.enable, {.data = true}, {.sentAt = ms(2)});
    auto third = harness.request(node.enable, {.data = true}, {.sentAt = ms(3)});
    harness.step(node, at(10));
    EXPECT_TRUE(first.replied());
    EXPECT_TRUE(second.replied());
    EXPECT_TRUE(third.queued()) << third.describe();
    harness.step(node, at(20));
    EXPECT_TRUE(third.replied()) << third.describe();
}

TEST_F(ServiceTables, ATableAStartRefusesIsRefused)
{
    this->load("[ports.servers.enable]\nservice = \"~/enable\"\nper_step = 0\n");
    Switch node;
    EXPECT_THROW(hypha::testing::StepHarness{node}, hypha::ConfigError);
}

TEST_F(ServiceTables, OutstandingAndTimeoutComeFromTheTable)
{
    this->load("[ports.clients.arming]\nservice = \"/arming\"\noutstanding = 2\ntimeout_ms = 50\n");
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    node.callNow = true;
    node.callsPerStep = 3;
    harness.step(node, at(0));
    EXPECT_EQ(
        node.statuses,
        (std::vector<hypha::CallStatus>{
            hypha::CallStatus::Sent, hypha::CallStatus::Sent, hypha::CallStatus::Busy
        })
    );
    harness.step(node, at(50));
    ASSERT_EQ(node.results.size(), 2U);
    EXPECT_EQ(node.results[0].outcome(), CallOutcome::Timeout);
    EXPECT_NE(
        node.results[0].description().find("timeout_ms of [ports.clients.arming]"),
        std::string::npos
    ) << node.results[0].description();
}

TEST(StepHarnessServices, ACallLeavesAfterItsStepAndItsOutcomeReachesALaterStep)
{
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    node.callNow = true;
    harness.step(node, at(0));
    ASSERT_EQ(node.statuses, std::vector<hypha::CallStatus>{hypha::CallStatus::Sent});
    ASSERT_EQ(harness.calls(node.arming).size(), 1U);
    EXPECT_TRUE(harness.calls(node.arming)[0].data);
    harness.step(node, at(10));
    EXPECT_TRUE(node.results.empty()) << "no outcome before the test gives one";
    EXPECT_TRUE(harness.calls(node.arming).empty()) << "calls() is the last step's";
    harness.answer(node.arming, {.success = true, .message = "armed"});
    harness.step(node, at(20));
    ASSERT_EQ(node.results.size(), 1U);
    ASSERT_TRUE(node.results[0].ok()) << node.results[0].description();
    EXPECT_EQ(node.results[0].response().message, "armed");
    EXPECT_EQ(node.results[0].sent(), ms(0));
}

TEST(StepHarnessServices, EveryOutcomeReachesTheStep)
{
    const std::vector<std::pair<CallOutcome, std::string>> rows{
        {CallOutcome::Refused, "refused: not now"},
        {CallOutcome::NoServer, "no server: not now"},
        {CallOutcome::NotSent, "not sent: not now"},
        {CallOutcome::ServerLost, "server lost: not now"},
        {CallOutcome::Timeout, "timeout not now"},
    };
    for (const auto& [outcome, text] : rows) {
        Arming                      node;
        hypha::testing::StepHarness harness{node};
        node.callNow = true;
        harness.step(node, at(0));
        harness.outcome(node.arming, outcome, "not now");
        harness.step(node, at(10));
        ASSERT_EQ(node.results.size(), 1U) << text;
        EXPECT_EQ(node.results[0].outcome(), outcome) << text;
        EXPECT_EQ(node.results[0].description(), text);
    }
}

TEST(StepHarnessServices, AnOutcomeIsReadByTheFirstStepAfterItsStamp)
{
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    node.callNow = true;
    harness.step(node, at(0));
    harness.answer(node.arming, {.success = true}, ms(10));
    harness.step(node, at(10));
    EXPECT_TRUE(node.results.empty()) << "an answer stamped at the step's time waits";
    harness.step(node, at(11));
    EXPECT_EQ(node.results.size(), 1U);
}

TEST(StepHarnessServices, AnOutcomeWithNoCallInFlightIsRefused)
{
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    EXPECT_THROW(harness.answer(node.arming, {.success = true}), std::logic_error);
    EXPECT_THROW(harness.outcome(node.arming, CallOutcome::Answered), std::logic_error);
    node.callNow = true;
    harness.step(node, at(0));
    harness.answer(node.arming, {.success = true});
    EXPECT_THROW(harness.answer(node.arming, {.success = true}), std::logic_error);
}

TEST(StepHarnessServices, ACallOfAFailedStepEndsNotSentInTheNextStep)
{
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    node.callNow = true;
    node.throws = true;
    EXPECT_THROW(harness.step(node, at(0)), std::runtime_error);
    EXPECT_TRUE(harness.calls(node.arming).empty()) << "a failed step sends nothing";
    node.throws = false;
    harness.step(node, at(10));
    ASSERT_EQ(node.results.size(), 1U);
    EXPECT_EQ(node.results[0].outcome(), CallOutcome::NotSent);
    EXPECT_NE(node.results[0].description().find("staged the call failed"), std::string::npos)
        << node.results[0].description();
}

TEST(StepHarnessServices, AFailedStepCarriesTheOutcomesItDidNotRead)
{
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    node.callNow = true;
    harness.step(node, at(0));
    harness.answer(node.arming, {.success = true});
    node.read = false;
    node.throws = true;
    EXPECT_THROW(harness.step(node, at(10)), std::runtime_error);
    node.read = true;
    node.throws = false;
    harness.step(node, at(20));
    ASSERT_EQ(node.results.size(), 1U);
    EXPECT_TRUE(node.results[0].ok()) << node.results[0].description();
    EXPECT_EQ(node.unread, 0U);
}

TEST(StepHarnessServices, ASuccessfulStepDropsAndCountsTheOutcomesItDidNotRead)
{
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    node.callNow = true;
    harness.step(node, at(0));
    harness.answer(node.arming, {.success = true});
    node.read = false;
    harness.step(node, at(10));
    node.read = true;
    harness.step(node, at(20));
    EXPECT_TRUE(node.results.empty());
    EXPECT_EQ(node.unread, 1U);
}

TEST(StepHarnessServices, ACallLeftOpenPastItsTimeoutEndsTimeout)
{
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    node.callNow = true;
    harness.step(node, at(0));
    harness.step(node, at(999));
    EXPECT_TRUE(node.results.empty());
    harness.step(node, at(1000));
    ASSERT_EQ(node.results.size(), 1U);
    EXPECT_EQ(node.results[0].outcome(), CallOutcome::Timeout);
    EXPECT_THROW(harness.answer(node.arming, {.success = true}), std::logic_error)
        << "the test's server let the call go";
    node.callNow = true;
    harness.step(node, at(1010));
    EXPECT_EQ(node.statuses.back(), hypha::CallStatus::Sent) << "its slot is free again";
}

TEST(StepHarnessServices, OneCallInFlightByDefault)
{
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    node.callNow = true;
    node.callsPerStep = 2;
    harness.step(node, at(0));
    EXPECT_EQ(
        node.statuses,
        (std::vector<hypha::CallStatus>{hypha::CallStatus::Sent, hypha::CallStatus::Busy})
    );
    node.callNow = true;
    node.callsPerStep = 1;
    harness.step(node, at(10));
    EXPECT_EQ(node.statuses.back(), hypha::CallStatus::Busy) << "the first is still in flight";
}

TEST(StepHarnessServices, AServicePortOutsideAStepIsStillRefused)
{
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    EXPECT_THROW(static_cast<void>(node.arming.call({.data = true})), std::logic_error);
}

}  // namespace
