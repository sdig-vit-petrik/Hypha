// Services through the C++ SDK: each call ends in exactly one outcome (FSR-42), a
// blocking call is refused on the run's thread (FSR-43), a deferred request is answered
// from another thread, refused when destroyed unanswered, and disconnected when it
// outlives its server.

#include <hypha_test_srvs/Echo.h>

#include "test_types.h"

#include <camera_control_msgs/SetExposure.h>
#include <gtest/gtest.h>
#include <std_srvs/SetBool.h>
#include <std_srvs/Trigger.h>

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <deque>
#include <functional>
#include <mutex>
#include <optional>
#include <string>
#include <thread>

/// Echo's requests are bounded at 64 bytes, so a longer text is TooLarge.
template <>
struct hypha::ServicePolicyOf<hypha_test_srvs::Echo>
{
    static inline const hypha::ServicePolicy value{
        "echoService", 4, 2, 64, 64, hypha::Duration::fromSeconds(1.0)
    };
};

namespace {

using hypha_test::testTopic;
using SetBool = std_srvs::SetBool;

constexpr hypha::Duration SHORT = hypha::Duration::fromMillis(150);

class ServiceFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        static_cast<void>(hypha::disableSimTime());
        this->node.emplace("service_test");
    }

    /// Run in short slices until @p done holds, for at most 5 s.
    void runUntil(const std::function<bool()>& done)
    {
        const auto until = std::chrono::steady_clock::now() + std::chrono::seconds(5);
        while (!done() && std::chrono::steady_clock::now() < until) {
            static_cast<void>(this->node->runFor(hypha::Duration::fromMillis(5)));
        }
    }

    /// The outcome of one call of @p client, run to completion.
    template <typename Srv>
    hypha::CallResult<Srv>
    callAndRun(hypha::ServiceClient<Srv>& client, const typename Srv::Request& request)
    {
        std::optional<hypha::CallResult<Srv>> result;
        const hypha::CallStatus               status =
            client.call(request, [&result](const hypha::CallResult<Srv>& r) {
                result = r;
            });
        EXPECT_EQ(status, hypha::CallStatus::Sent);
        this->runUntil([&result] {
            return result.has_value();
        });
        EXPECT_TRUE(result.has_value()) << "no outcome within 5 s";
        return result.value_or(hypha::CallResult<Srv>::notSent(hypha::CallStatus::Busy));
    }

    std::string name(const std::string& suffix) const
    {
        return testTopic("/" + suffix);
    }

    std::optional<hypha::Node> node;
};

TEST_F(ServiceFixture, AnAnsweredCallDeliversTheResponseAtALaterTurn)
{
    auto server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("arming")}, [](hypha::Request<SetBool>& request) {
            static_cast<void>(request.reply({.success = request->data, .message = "armed"}));
        }
    );
    auto client = this->node->serviceClient<SetBool>(hypha::Service{this->name("arming")});

    const auto result = this->callAndRun(client, {.data = true});
    ASSERT_TRUE(result.ok()) << result.description();
    EXPECT_EQ(result.outcome(), hypha::CallOutcome::Answered);
    EXPECT_TRUE(result.response().success);
    EXPECT_EQ(result.response().message, "armed");
    EXPECT_TRUE(result.mayHaveRun());
    EXPECT_FALSE(result.safeToRetry());
}

TEST_F(ServiceFixture, ARefusalCarriesItsReason)
{
    auto server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("refusing")}, [](hypha::Request<SetBool>& request) {
            static_cast<void>(request.refuse("not armed"));
        }
    );
    auto client = this->node->serviceClient<SetBool>(hypha::Service{this->name("refusing")});

    const auto result = this->callAndRun(client, {.data = true});
    EXPECT_EQ(result.outcome(), hypha::CallOutcome::Refused);
    EXPECT_EQ(hypha::describe(result), "refused: not armed");
    EXPECT_TRUE(result.safeToRetry());
    EXPECT_THROW(static_cast<void>(result.response()), std::logic_error);
}

TEST_F(ServiceFixture, ARequestTheHandlerLeavesUnansweredIsRefused)
{
    auto server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("silent")}, [](hypha::Request<SetBool>&) {
        }
    );
    auto client = this->node->serviceClient<SetBool>(hypha::Service{this->name("silent")});

    const auto result = this->callAndRun(client, {.data = true});
    EXPECT_EQ(result.outcome(), hypha::CallOutcome::Refused);
    EXPECT_NE(result.description().find("did not answer"), std::string::npos)
        << result.description();
}

TEST_F(ServiceFixture, ACallWithoutAServerEndsNoServerAtItsDeadline)
{
    auto client = this->node->serviceClient<SetBool>(
        hypha::Service{this->name("nobody")}, hypha::timeout(SHORT)
    );

    const auto result = this->callAndRun(client, {.data = true});
    EXPECT_EQ(result.outcome(), hypha::CallOutcome::NoServer);
    EXPECT_NE(result.description().find("within 150 ms (set in code)"), std::string::npos)
        << result.description();
    EXPECT_TRUE(result.safeToRetry());
    EXPECT_FALSE(client.pending());
}

TEST_F(ServiceFixture, ARequestAboveItsBoundIsTooLargeAndABlockingOneIsNotSent)
{
    auto client =
        this->node->serviceClient<hypha_test_srvs::Echo>(hypha::Service{this->name("echo")});
    const hypha_test_srvs::Echo::Request large{.text = std::string(100, 'x')};
    EXPECT_EQ(client.call(large, {}), hypha::CallStatus::TooLarge);

    auto blocking = this->node->serviceClient<hypha_test_srvs::Echo>(
        hypha::Service{this->name("echo")}, hypha::blocking
    );
    const auto result = blocking.callAndWait(large);
    EXPECT_EQ(result.outcome(), hypha::CallOutcome::NotSent);
    EXPECT_TRUE(result.safeToRetry());
    EXPECT_NE(result.description().find("too large"), std::string::npos) << result.description();
}

TEST_F(ServiceFixture, ARequestTheServerHoldsPastTheTimeoutEndsTimeout)
{
    hypha::Deferred<SetBool> held;
    auto                     server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("slow")}, [&held](hypha::Request<SetBool>& request) {
            held = request.defer();
        }
    );
    auto client = this->node->serviceClient<SetBool>(
        hypha::Service{this->name("slow")}, hypha::timeout(SHORT)
    );

    const auto result = this->callAndRun(client, {.data = true});
    EXPECT_EQ(result.outcome(), hypha::CallOutcome::Timeout);
    EXPECT_EQ(result.description(), "timeout after 150 ms (set in code)");
    EXPECT_TRUE(result.mayHaveRun());
    EXPECT_TRUE(static_cast<bool>(held));
}

TEST_F(ServiceFixture, AServerDestroyedWithTheRequestEndsServerLostAndItsDeferredIsDisconnected)
{
    hypha::Deferred<SetBool>            held;
    std::optional<hypha::ServiceServer> server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("leaving")}, [&held](hypha::Request<SetBool>& request) {
            held = request.defer();
        }
    );
    auto client = this->node->serviceClient<SetBool>(hypha::Service{this->name("leaving")});
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
    this->runUntil([&held] {
        return static_cast<bool>(held);
    });
    ASSERT_TRUE(static_cast<bool>(held));

    server.reset();
    EXPECT_EQ(held.reply({.success = true, .message = "too late"}), hypha::Status::Disconnected);
    this->runUntil([&result] {
        return result.has_value();
    });
    ASSERT_TRUE(result.has_value());
    EXPECT_EQ(result->outcome(), hypha::CallOutcome::ServerLost);
    EXPECT_TRUE(result->mayHaveRun());
    EXPECT_FALSE(result->safeToRetry());
}

TEST_F(ServiceFixture, AHandlerThatThrowsLosesTheCallAndFailsTheRun)
{
    auto server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("throwing")}, [](hypha::Request<SetBool>&) {
            throw std::runtime_error("device on fire");
        }
    );
    auto client = this->node->serviceClient<SetBool>(hypha::Service{this->name("throwing")});
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
    bool failed = false;
    for (int turn = 0; turn < 1000 && !result; ++turn) {
        const hypha::RunResult run = this->node->runFor(hypha::Duration::fromMillis(5));
        failed = failed || run.status() == hypha::Status::Panic;
    }
    EXPECT_TRUE(failed) << "a handler's throw fails the run";
    ASSERT_TRUE(result.has_value());
    EXPECT_EQ(result->outcome(), hypha::CallOutcome::ServerLost);
    EXPECT_NE(result->description().find("handler failed"), std::string::npos)
        << result->description();
}

TEST_F(ServiceFixture, ADeferredRequestIsAnsweredFromAnotherThread)
{
    std::thread worker;
    auto        server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("worker")}, [&worker](hypha::Request<SetBool>& request) {
            worker = std::thread([deferred = request.defer()]() mutable {
                EXPECT_EQ(
                    deferred.reply({.success = true, .message = "worker"}), hypha::Status::Ok
                );
                EXPECT_FALSE(static_cast<bool>(deferred));
            });
        }
    );
    auto client = this->node->serviceClient<SetBool>(hypha::Service{this->name("worker")});

    const auto result = this->callAndRun(client, {.data = true});
    worker.join();
    ASSERT_TRUE(result.ok()) << result.description();
    EXPECT_EQ(result.response().message, "worker");
}

TEST_F(ServiceFixture, ADeferredRequestDestroyedUnansweredIsRefusedNeverSilent)
{
    auto server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("dropping")}, [](hypha::Request<SetBool>& request) {
            static_cast<void>(request.defer());
        }
    );
    auto client = this->node->serviceClient<SetBool>(hypha::Service{this->name("dropping")});

    const auto result = this->callAndRun(client, {.data = true});
    EXPECT_EQ(result.outcome(), hypha::CallOutcome::Refused);
    EXPECT_NE(result.description().find("destroyed unanswered"), std::string::npos)
        << result.description();
}

TEST_F(ServiceFixture, ACallMadeBeforeItsServerExistsIsSentWhenOneAppears)
{
    auto client = this->node->serviceClient<SetBool>(hypha::Service{this->name("late")});
    EXPECT_FALSE(client.ready());
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
    EXPECT_TRUE(client.pending());
    static_cast<void>(this->node->runFor(hypha::Duration::fromMillis(20)));
    ASSERT_FALSE(result.has_value()) << result->description();

    auto server = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("late")}, [](hypha::Request<SetBool>& request) {
            static_cast<void>(request.reply({.success = true, .message = "here now"}));
        }
    );
    EXPECT_TRUE(client.ready());
    this->runUntil([&result] {
        return result.has_value();
    });
    ASSERT_TRUE(result.has_value());
    ASSERT_TRUE(result->ok()) << result->description();
    EXPECT_EQ(result->response().message, "here now");
}

TEST_F(ServiceFixture, ABlockingCallOnTheRunThreadIsRefusedWithWhatToUseInstead)
{
    auto blocking =
        this->node->serviceClient<SetBool>(hypha::Service{this->name("blocked")}, hypha::blocking);
    std::string refusal;
    auto        timer = this->node->timer(hypha::Duration::fromMillis(1), [&blocking, &refusal] {
        if (!refusal.empty()) {
            return;
        }
        try {
            static_cast<void>(blocking.callAndWait({.data = true}));
            refusal = "not refused";
        } catch (const std::runtime_error& error) {
            refusal = error.what();
        }
    });
    this->runUntil([&refusal] {
        return !refusal.empty();
    });
    EXPECT_NE(refusal.find("InvalidState"), std::string::npos) << refusal;
    EXPECT_NE(refusal.find("call(request, onResult)"), std::string::npos) << refusal;
}

TEST_F(ServiceFixture, ASecondServerOfTheNameIsRefusedNamingTheHolder)
{
    auto first = this->node->advertiseService<SetBool>(
        hypha::Service{this->name("taken")}, [](hypha::Request<SetBool>&) {
        }
    );
    try {
        static_cast<void>(this->node->advertiseService<std_srvs::Trigger>(
            hypha::Service{this->name("taken")}, [](hypha::Request<std_srvs::Trigger>&) {
            }
        ));
        FAIL() << "a second server of the name was created";
    } catch (const std::runtime_error& error) {
        const std::string text = error.what();
        EXPECT_NE(text.find("ResourceLimit"), std::string::npos) << text;
        EXPECT_NE(text.find("already served as std_srvs/SetBool"), std::string::npos) << text;
    }
}

/// The camera's shape (native-services.md §8.3): a worker thread owns the device and
/// answers the setters between two grabs; the handlers only hand requests over.
class FakeCamera : public hypha::ApplicationNode
{
  public:
    ~FakeCamera() override
    {
        {
            const std::lock_guard<std::mutex> lock(this->mutex_);
            this->stopping_ = true;
        }
        this->wake_.notify_one();
        if (this->worker_.joinable()) {
            this->worker_.join();
        }
    }

    void setup(hypha::Node& node) override
    {
        this->enable_ = node.advertiseService<SetBool>(
            hypha::Service{testTopic("/camera/set_enabled")},
            [this](hypha::Request<SetBool>& request) {
                this->push(Command{request->data, 0.0F, request.defer(), {}});
            }
        );
        this->exposure_ = node.advertiseService<camera_control_msgs::SetExposure>(
            hypha::Service{testTopic("/camera/set_exposure")},
            [this](hypha::Request<camera_control_msgs::SetExposure>& request) {
                this->push(Command{std::nullopt, request->target_exposure, {}, request.defer()});
            }
        );
        this->worker_ = std::thread([this] {
            this->grabLoop();
        });
    }

  private:
    struct Command
    {
        std::optional<bool>                               enable;
        float                                             exposure{0.0F};
        hypha::Deferred<SetBool>                          enableReply;
        hypha::Deferred<camera_control_msgs::SetExposure> exposureReply;
    };

    void push(Command command)
    {
        {
            const std::lock_guard<std::mutex> lock(this->mutex_);
            this->commands_.push_back(std::move(command));
        }
        this->wake_.notify_one();
    }

    /// The worker: the only thread that touches the device.
    void grabLoop()
    {
        for (;;) {
            std::unique_lock<std::mutex> lock(this->mutex_);
            this->wake_.wait(lock, [this] {
                return this->stopping_ || !this->commands_.empty();
            });
            if (this->stopping_) {
                return;
            }
            Command command = std::move(this->commands_.front());
            this->commands_.pop_front();
            lock.unlock();
            if (command.enable) {
                this->deviceEnabled_ = *command.enable;
                static_cast<void>(command.enableReply.reply(
                    {.success = true, .message = this->deviceEnabled_ ? "streaming" : "stopped"}
                ));
            } else {
                // The device reaches what its range allows: at most 10 ms.
                const float reached = std::min(command.exposure, 10000.0F);
                static_cast<void>(command.exposureReply.reply(
                    {.reached_exposure = reached, .success = reached == command.exposure}
                ));
            }
        }
    }

    hypha::ServiceServer    enable_;
    hypha::ServiceServer    exposure_;
    std::mutex              mutex_;
    std::condition_variable wake_;
    std::deque<Command>     commands_;
    bool                    stopping_{false};
    bool                    deviceEnabled_{false};
    std::thread             worker_;
};

TEST_F(ServiceFixture, TheCameraWorkerAnswersSettersCalledByABlockingClient)
{
    FakeCamera camera;
    camera.setup(*this->node);
    std::atomic<bool>                                                  done{false};
    std::optional<hypha::CallResult<SetBool>>                          enabled;
    std::optional<hypha::CallResult<camera_control_msgs::SetExposure>> exposed;
    std::thread                                                        operatorThread([&] {
        auto enable = this->node->serviceClient<SetBool>(
            hypha::Service{testTopic("/camera/set_enabled")}, hypha::blocking
        );
        auto exposure = this->node->serviceClient<camera_control_msgs::SetExposure>(
            hypha::Service{testTopic("/camera/set_exposure")}, hypha::blocking
        );
        EXPECT_TRUE(enable.waitForServer(hypha::Duration::fromSeconds(2.0)));
        enabled = enable.callAndWait({.data = true});
        exposed = exposure.callAndWait({.target_exposure = 20000.0F});
        done = true;
    });
    this->runUntil([&done] {
        return done.load();
    });
    operatorThread.join();
    ASSERT_TRUE(enabled.has_value() && enabled->ok()) << (enabled ? enabled->description() : "");
    EXPECT_EQ(enabled->response().message, "streaming");
    ASSERT_TRUE(exposed.has_value() && exposed->ok()) << (exposed ? exposed->description() : "");
    EXPECT_FLOAT_EQ(exposed->response().reached_exposure, 10000.0F);
    EXPECT_FALSE(exposed->response().success);
}

}  // namespace
