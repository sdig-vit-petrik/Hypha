#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include <hypha_msgs/std_msgs/Float64.h>

#include <gtest/gtest.h>

#include <csignal>
#include <filesystem>
#include <fstream>
#include <memory>
#include <string>

namespace {

/// The step keeps `lifetime`, so a test can see when the cycle releases its step.
struct ShutdownStep
{
    std::shared_ptr<int> lifetime;

    void operator()(const hypha::CycleTime&) const
    {
    }
};

struct ShutdownDeleter
{
    int* calls;

    void operator()(int* value) const
    {
        ++*this->calls;
        hypha::backend::shutdown();
        delete value;
    }
};

/// One input and one output on a Cycle. The cycle is declared first, so the ports die before it.
struct ShutdownCycle
{
    ShutdownCycle(const std::string& name, hypha::Topic topic, const std::string& outputParameter)
        : cycle(name)
        , input(cycle, std::move(topic), hypha::unsupervised)
        , output(cycle, outputParameter)
    {
        this->cycle.setRate(hypha::Hz(10.0));
    }

    hypha::Cycle                     cycle;
    hypha::Latest<std_msgs::Float64> input;
    hypha::Output<std_msgs::Float64> output;
};

class CycleShutdownFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_cycle_shutdown.toml";
        std::ofstream output(this->path);
        for (const char* name : {"out_first", "out_second", "out_old", "out_recursive"}) {
            output << "[ports.outputs." << name << "]\ntopic='/shutdown_output/" << name << "'\n";
        }
        output.close();
    }

    void TearDown() override
    {
        std::filesystem::remove(this->path);
    }

    std::filesystem::path path;
};

}  // namespace

/// A stop signal is for the rest of the process, so the test that raises one runs in a
/// process of its own (CMakeLists.txt: the `*StopSignalProcess` suites).
class CycleShutdownStopSignalProcess : public CycleShutdownFixture
{
};

TEST_F(CycleShutdownStopSignalProcess, NodeDestructionReleasesCyclesBeforeTheSession)
{
    auto node = std::make_unique<hypha::Node>("repeated_cycle_shutdown", this->path.string());
    auto lifetime = std::make_shared<int>(0);
    const std::weak_ptr<int> released = lifetime;
    ShutdownCycle            first("cycle", hypha::Topic{"/shutdown_input"}, "out_first");
    ShutdownCycle            second("second_cycle", hypha::Topic{"/shutdown_input"}, "out_second");
    ASSERT_TRUE(first.cycle.start(*node, ShutdownStep{lifetime}).has_value());
    ASSERT_TRUE(second.cycle.start(*node, ShutdownStep{lifetime}).has_value());
    lifetime.reset();

    ASSERT_TRUE(first.cycle.health().has_value());
    ASSERT_TRUE(node->runFor(hypha::Duration::fromNanos(1000000)).ok());
    ASSERT_EQ(std::raise(SIGINT), 0);
    ASSERT_TRUE(node->run().ok());

    testing::internal::CaptureStderr();
    node.reset();
    EXPECT_TRUE(released.expired());
    const auto health = first.cycle.health();
    ASSERT_FALSE(health.has_value());
    EXPECT_EQ(health.error(), hypha::Status::InvalidState);
    EXPECT_TRUE(testing::internal::GetCapturedStderr().empty());
}

TEST_F(CycleShutdownFixture, StepDestructionDuringReleaseIsIgnored)
{
    int  calls = 0;
    auto node = std::make_unique<hypha::Node>("recursive_cycle_shutdown", this->path.string());
    std::shared_ptr<int> lifetime(new int(0), ShutdownDeleter{&calls});
    ShutdownCycle recursive("cycle", hypha::Topic{"/recursive_shutdown_input"}, "out_recursive");
    ASSERT_TRUE(recursive.cycle.start(*node, ShutdownStep{lifetime}).has_value());
    lifetime.reset();
    testing::internal::CaptureStderr();
    node.reset();
    EXPECT_EQ(calls, 1);
    EXPECT_TRUE(testing::internal::GetCapturedStderr().empty());
    EXPECT_FALSE(hypha::ok());
}
