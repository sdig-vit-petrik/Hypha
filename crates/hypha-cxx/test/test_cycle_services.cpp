// A server and a client in a cycle (docs/design/native-services.md §2, §3), on host time:
// a step answers what it read and its answers leave after it; a request a step never read
// stays queued and one it read and left alone is refused; a call leaves after its step and
// its outcome comes in a later step; a failed step loses what it read and ends what it
// called NotSent; every call ends in exactly one outcome (FSR-42).

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"

#include "test_types.h"

#include <gtest/gtest.h>
#include <std_srvs/SetBool.h>

#include <chrono>
#include <functional>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

using SetBool = std_srvs::SetBool;
using namespace hypha::literals;

class CycleServiceFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        static_cast<void>(hypha::disableSimTime());
        this->node.emplace("cycle_service_test");
    }

    /// Run in short slices until @p done holds, for at most 5 s; a failed run goes on.
    void runUntil(const std::function<bool()>& done)
    {
        const auto until = std::chrono::steady_clock::now() + std::chrono::seconds(5);
        while (!done() && std::chrono::steady_clock::now() < until) {
            static_cast<void>(this->node->runFor(hypha::Duration::fromMillis(2)));
        }
    }

    std::string name(const std::string& suffix) const
    {
        return hypha_test::testTopic("/" + suffix);
    }

    /// The outcome of one call of a plain client of @p service, run to completion.
    hypha::CallResult<SetBool> callAndRun(const std::string& service, bool data)
    {
        auto client = this->node->serviceClient<SetBool>(hypha::Service{service});
        std::optional<hypha::CallResult<SetBool>> result;
        const hypha::CallStatus                   status =
            client.call({.data = data}, [&result](const hypha::CallResult<SetBool>& r) {
                result = r;
            });
        EXPECT_EQ(status, hypha::CallStatus::Sent);
        this->runUntil([&result] {
            return result.has_value();
        });
        EXPECT_TRUE(result.has_value()) << "no outcome within 5 s";
        return result.value_or(hypha::CallResult<SetBool>::notSent(hypha::CallStatus::Busy));
    }

    std::optional<hypha::Node> node;
};

/// A server cycle whose step does what `serve` says with each request it reads.
struct ServerCycle
{
    explicit ServerCycle(const std::string& service)
        : cycle("server"), port(cycle, hypha::Service{service})
    {
        this->cycle.setRate(hypha::Hz(500.0));
    }

    hypha::Cycle                                  cycle;
    hypha::Requests<SetBool>                      port;
    bool                                          read{true};
    std::function<void(hypha::Request<SetBool>&)> serve;
    int                                           steps{0};
    bool                                          throwAfterReading{false};

    struct Step
    {
        ServerCycle* self;

        void operator()(const hypha::CycleTime&) const
        {
            self->steps += 1;
            if (!self->read) {
                return;
            }
            for (hypha::Request<SetBool>& request : self->port.requests()) {
                if (self->serve) {
                    self->serve(request);
                }
            }
            if (self->throwAfterReading) {
                self->throwAfterReading = false;
                throw std::runtime_error("the server step failed on purpose");
            }
        }
    };

    void start(hypha::Node& node)
    {
        const auto started = this->cycle.start(node, Step{this});
        ASSERT_TRUE(started.has_value()) << started.error();
    }
};

TEST_F(CycleServiceFixture, AStepAnswersTheRequestsItReads)
{
    ServerCycle server(this->name("cycle_arming"));
    server.serve = [](hypha::Request<SetBool>& request) {
        EXPECT_EQ(
            request.reply({.success = request->data, .message = "armed in a step"}),
            hypha::Status::Ok
        );
    };
    server.start(*this->node);

    const auto result = this->callAndRun(this->name("cycle_arming"), true);
    ASSERT_TRUE(result.ok()) << result.description();
    EXPECT_TRUE(result.response().success);
    EXPECT_EQ(result.response().message, "armed in a step");
}

TEST_F(CycleServiceFixture, ARequestNoStepReadsWaitsAndOneAStepReadAndLeftIsRefused)
{
    ServerCycle server(this->name("cycle_waiting"));
    server.read = false;
    server.start(*this->node);
    auto client = this->node->serviceClient<SetBool>(hypha::Service{this->name("cycle_waiting")});
    std::optional<hypha::CallResult<SetBool>> result;
    ASSERT_EQ(
        client.call(
            {.data = true},
            [&result](const hypha::CallResult<SetBool>& r) {
                result = r;
            }
        ),
        hypha::CallStatus::Sent
    );
    const int before = server.steps;
    this->runUntil([&] {
        return server.steps > before + 20;
    });
    EXPECT_FALSE(result.has_value()) << "a request no step read is not refused";

    server.read = true;  // read now, and leave it unanswered
    this->runUntil([&result] {
        return result.has_value();
    });
    ASSERT_TRUE(result.has_value());
    EXPECT_EQ(result->outcome(), hypha::CallOutcome::Refused);
    EXPECT_NE(result->description().find("did not answer"), std::string::npos)
        << result->description();
}

TEST_F(CycleServiceFixture, AFailedServerStepLosesTheRequestsItRead)
{
    ServerCycle server(this->name("cycle_failing"));
    server.serve = [](hypha::Request<SetBool>& request) {
        static_cast<void>(request.reply({.success = true, .message = "never sent"}));
    };
    server.throwAfterReading = true;
    server.start(*this->node);

    const auto result = this->callAndRun(this->name("cycle_failing"), true);
    EXPECT_EQ(result.outcome(), hypha::CallOutcome::ServerLost) << result.description();
    EXPECT_NE(result.description().find("server step failed"), std::string::npos)
        << result.description();
    EXPECT_TRUE(result.mayHaveRun());
}

TEST_F(CycleServiceFixture, ARequestDeferredInAStepIsAnsweredInALaterStep)
{
    ServerCycle                             server(this->name("cycle_deferred"));
    std::optional<hypha::Deferred<SetBool>> held;
    int                                     deferredAt = 0;
    server.serve = [&](hypha::Request<SetBool>& request) {
        held.emplace(request.defer());
        deferredAt = server.steps;
    };
    server.start(*this->node);
    auto client = this->node->serviceClient<SetBool>(hypha::Service{this->name("cycle_deferred")});
    std::optional<hypha::CallResult<SetBool>> result;
    ASSERT_EQ(
        client.call(
            {.data = true},
            [&result](const hypha::CallResult<SetBool>& r) {
                result = r;
            }
        ),
        hypha::CallStatus::Sent
    );
    this->runUntil([&] {
        return held.has_value();
    });
    ASSERT_TRUE(held.has_value());
    // Three steps later, from the step of another cycle of the node.
    hypha::Cycle answering("answering");
    answering.setRate(hypha::Hz(500.0));

    struct Answer
    {
        ServerCycle*                             server;
        int                                      at;
        std::optional<hypha::Deferred<SetBool>>* held;

        void operator()(const hypha::CycleTime&) const
        {
            if (server->steps >= at + 3 && *held && **held) {
                EXPECT_EQ((*held)->reply({.success = true, .message = "later"}), hypha::Status::Ok);
            }
        }
    };

    ASSERT_TRUE(answering.start(*this->node, Answer{&server, deferredAt, &held}).has_value());
    this->runUntil([&result] {
        return result.has_value();
    });
    ASSERT_TRUE(result.has_value());
    ASSERT_TRUE(result->ok()) << result->description();
    EXPECT_EQ(result->response().message, "later");
}

/// A client cycle that calls when `callNow` and records what each step read.
struct ClientCycle
{
    explicit ClientCycle(const std::string& service)
        : cycle("client"), port(cycle, hypha::Service{service})
    {
        this->cycle.setRate(hypha::Hz(500.0));
    }

    hypha::Cycle                                            cycle;
    hypha::Calls<SetBool>                                   port;
    int                                                     callNow{0};
    bool                                                    throwAfterCalling{false};
    std::vector<hypha::CallStatus>                          statuses;
    std::vector<int>                                        calledAt;
    std::vector<std::pair<int, hypha::CallResult<SetBool>>> results;
    int                                                     steps{0};

    struct Step
    {
        ClientCycle* self;

        void operator()(const hypha::CycleTime&) const
        {
            self->steps += 1;
            for (auto& result : self->port.results()) {
                self->results.emplace_back(self->steps, std::move(result));
            }
            for (; self->callNow > 0; --self->callNow) {
                self->statuses.push_back(self->port.call({.data = true}));
                self->calledAt.push_back(self->steps);
            }
            if (self->throwAfterCalling) {
                self->throwAfterCalling = false;
                throw std::runtime_error("the client step failed on purpose");
            }
        }
    };

    void start(hypha::Node& node)
    {
        const auto started = this->cycle.start(node, Step{this});
        ASSERT_TRUE(started.has_value()) << started.error();
    }
};

TEST_F(CycleServiceFixture, ACallLeavesAfterItsStepAndALaterStepReadsItsOutcome)
{
    auto server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("cycle_called")}, [](hypha::Request<SetBool>& request) {
            static_cast<void>(request.reply({.success = request->data, .message = "plain"}));
        }
    );
    ClientCycle client(this->name("cycle_called"));
    client.callNow = 2;  // one Calls port has one call in flight by default: the second is Busy
    client.start(*this->node);
    this->runUntil([&] {
        return !client.results.empty();
    });
    ASSERT_EQ(client.statuses.size(), 2U);
    EXPECT_EQ(client.statuses[0], hypha::CallStatus::Sent);
    EXPECT_EQ(client.statuses[1], hypha::CallStatus::Busy);
    EXPECT_EQ(client.port.lastStatus(), hypha::CallStatus::Busy);
    ASSERT_EQ(client.results.size(), 1U);
    EXPECT_GT(client.results[0].first, client.calledAt[0])
        << "an outcome is an input of a later step";
    ASSERT_TRUE(client.results[0].second.ok()) << client.results[0].second.description();
    EXPECT_EQ(client.results[0].second.response().message, "plain");
}

TEST_F(CycleServiceFixture, AFailedClientStepEndsItsCallNotSentInTheNextStep)
{
    int  served = 0;
    auto server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("cycle_unsent")}, [&served](hypha::Request<SetBool>& request) {
            served += 1;
            static_cast<void>(request.reply({.success = true}));
        }
    );
    ClientCycle client(this->name("cycle_unsent"));
    client.callNow = 1;
    client.throwAfterCalling = true;
    client.start(*this->node);
    this->runUntil([&] {
        return !client.results.empty();
    });
    ASSERT_EQ(client.results.size(), 1U);
    const auto& [step, result] = client.results[0];
    EXPECT_EQ(result.outcome(), hypha::CallOutcome::NotSent) << result.description();
    EXPECT_NE(result.description().find("staged the call failed"), std::string::npos)
        << result.description();
    EXPECT_TRUE(result.safeToRetry());
    EXPECT_EQ(step, client.calledAt[0] + 1);
    EXPECT_EQ(served, 0);
}

TEST_F(CycleServiceFixture, AClientAndItsServerInOneNodeCallEachOther)
{
    ServerCycle server(this->name("cycle_pair"));
    server.serve = [](hypha::Request<SetBool>& request) {
        static_cast<void>(request.reply({.success = !request->data, .message = "pair"}));
    };
    server.start(*this->node);
    ClientCycle client(this->name("cycle_pair"));
    client.callNow = 1;
    client.start(*this->node);
    this->runUntil([&] {
        return !client.results.empty();
    });
    ASSERT_EQ(client.results.size(), 1U);
    ASSERT_TRUE(client.results[0].second.ok()) << client.results[0].second.description();
    EXPECT_FALSE(client.results[0].second.response().success);
    EXPECT_EQ(client.results[0].second.response().message, "pair");
}

TEST_F(CycleServiceFixture, AServicePortOutsideAStepIsRefused)
{
    ServerCycle server(this->name("cycle_outside"));
    ClientCycle client(this->name("cycle_outside"));
    EXPECT_THROW(static_cast<void>(server.port.requests()), std::logic_error);
    EXPECT_THROW(static_cast<void>(client.port.call({.data = true})), std::logic_error);
    EXPECT_THROW(static_cast<void>(client.port.result()), std::logic_error);
}

}  // namespace
