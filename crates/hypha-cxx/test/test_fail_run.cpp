// failRun() ends the run as failed and holds the outputs of the step it is called in.

#include "hypha/cycle_node.h"
#include <hypha_msgs/std_msgs/Float64.h>

#include "test_types.h"

#include <gtest/gtest.h>

#include <atomic>
#include <filesystem>
#include <fstream>
#include <optional>
#include <string>
#include <thread>

using namespace hypha::literals;
using Float64 = std_msgs::Float64;

namespace {

/// A host-time node with two output tables, `before` and `after`, on test topics.
class FailRunFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_fail_run_test.toml";
        std::ofstream output(this->path);
        output << "[ports.outputs.before]\ntopic='" << hypha_test::testTopic("/before") << "'\n";
        output << "[ports.outputs.after]\ntopic='" << hypha_test::testTopic("/after") << "'\n";
        output.close();
        ASSERT_TRUE(hypha::disableSimTime());
        this->node.emplace("fail_run_test", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

/// Sends on `before`, fails, sends on `after`, in its first step.
class FailsInItsStep final : public hypha::CycleNode
{
  public:
    hypha::Output<Float64> before{*this, "before"};
    hypha::Output<Float64> after{*this, "after"};
    int                    steps{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        this->before.send(Float64{1.0});
        this->failRun("the step refused");
        this->after.send(Float64{2.0});
    }
};

/// Counts its steps and sends on `before` in each.
class CountsSteps final : public hypha::CycleNode
{
  public:
    hypha::Output<Float64> before{*this, "before"};
    hypha::Output<Float64> after{*this, "after"};
    std::atomic<int>       steps{0};

    void step(const hypha::CycleTime&) override
    {
        ++this->steps;
        this->before.send(Float64{1.0});
    }

    void fail(const std::string& reason)
    {
        this->failRun(reason);
    }
};

/// Calls CountsSteps::fail once the node has stepped at least once.
struct FailAfterAStep
{
    CountsSteps* app;

    void operator()() const
    {
        while (this->app->steps.load() == 0) {
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
        this->app->fail("a worker refused");
    }
};

/// Calls hypha::requestStop() once the node has stepped more than `stepsBefore` times.
struct StopAfterAStep
{
    CountsSteps* app;
    int          stepsBefore;

    void operator()() const
    {
        while (this->app->steps.load() <= this->stepsBefore) {
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
        hypha::requestStop();
    }
};

void expectApplicationFailure(const hypha::RunResult& result)
{
    EXPECT_FALSE(result.ok());
    EXPECT_EQ(result.reason(), hypha::StopReason::RuntimeFailed);
    EXPECT_EQ(result.status(), hypha::Status::ApplicationFailed);
}

}  // namespace

TEST_F(FailRunFixture, FailRunInAStepSendsNoneOfTheStepsOutputsAndFailsTheRunResult)
{
    hypha::Node probe;
    auto beforeSub = probe.subscribe<Float64>(hypha::Topic{hypha_test::testTopic("/before")});
    auto afterSub = probe.subscribe<Float64>(hypha::Topic{hypha_test::testTopic("/after")});
    FailsInItsStep app;
    app.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(app.start(*this->node).has_value());

    ::testing::internal::CaptureStderr();
    const hypha::RunResult result = this->node->run();
    const std::string      output = ::testing::internal::GetCapturedStderr();

    expectApplicationFailure(result);
    EXPECT_EQ(app.steps, 1);
    EXPECT_FALSE(beforeSub.latestSample().has_value());
    EXPECT_FALSE(afterSub.latestSample().has_value());
    EXPECT_NE(output.find("application failed: the step refused"), std::string::npos) << output;
}

TEST_F(FailRunFixture, FailRunFromAThreadFailsTheResultAndEndsTheRun)
{
    CountsSteps app;
    app.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(app.start(*this->node).has_value());

    std::thread            worker(FailAfterAStep{&app});
    const hypha::RunResult result = this->node->run();
    worker.join();
    const int stepsAtTheEnd = app.steps.load();

    expectApplicationFailure(result);
    // The request is consumed: a later run steps again and ends as its own stop says.
    std::thread            stopper(StopAfterAStep{&app, stepsAtTheEnd});
    const hypha::RunResult next = this->node->run();
    stopper.join();
    EXPECT_TRUE(next.ok());
    EXPECT_GT(app.steps.load(), stepsAtTheEnd);
}

TEST_F(FailRunFixture, AFailureRequestedBeforeTheRunFailsItAtOnce)
{
    hypha::Node probe;
    auto beforeSub = probe.subscribe<Float64>(hypha::Topic{hypha_test::testTopic("/before")});
    CountsSteps app;
    app.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(app.start(*this->node).has_value());

    app.fail("refused before the run");
    const hypha::RunResult result = this->node->runFor(1000_ms);

    expectApplicationFailure(result);
    EXPECT_EQ(app.steps.load(), 0);
    EXPECT_FALSE(beforeSub.latestSample().has_value());
}

TEST_F(FailRunFixture, RequestStopStillEndsTheRunAsOk)
{
    // The probe sees the steps' outputs here, so its silence above is the failure's.
    hypha::Node probe;
    auto beforeSub = probe.subscribe<Float64>(hypha::Topic{hypha_test::testTopic("/before")});
    CountsSteps app;
    app.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(app.start(*this->node).has_value());

    std::thread            stopper(StopAfterAStep{&app, 0});
    const hypha::RunResult result = this->node->run();
    stopper.join();

    EXPECT_TRUE(result.ok());
    EXPECT_EQ(result.reason(), hypha::StopReason::Requested);
    EXPECT_TRUE(beforeSub.latestSample().has_value());
}
