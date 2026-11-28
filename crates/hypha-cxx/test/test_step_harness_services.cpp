// The service ports in a testing::StepHarness: the test is the client of a Requests port
// and the server of a Calls port. What a step reads, answers, sends and carries is decided
// in hypha_node (crates/hypha_node/tests/service_step.rs); these cases are the C++ wiring:
// probes, deferred handles, the port tables and a port used outside a step.

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
    bool                                          throws{false};
    std::function<void(hypha::Request<SetBool>&)> serve = [](hypha::Request<SetBool>& r) {
        static_cast<void>(r.reply({.success = r->data, .message = "from " + r.clientNode()}));
    };
    hypha::Deferred<SetBool> held;

    void step(const hypha::CycleTime&) override
    {
        for (hypha::Request<SetBool>& request : this->enable.requests()) {
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

TEST(StepHarnessServices, AServicePortOutsideAStepIsStillRefused)
{
    Arming                      node;
    hypha::testing::StepHarness harness{node};
    EXPECT_THROW(static_cast<void>(node.arming.call({.data = true})), std::logic_error);
}

}  // namespace
