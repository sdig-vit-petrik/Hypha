#include "hypha/cycle_node.h"
#include "hypha/hypha.h"

#include <gtest/gtest.h>
#include <std_msgs/Float64.h>

#include <atomic>
#include <chrono>
#include <cstddef>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <stdexcept>
#include <string>
#include <thread>
#include <utility>

namespace {

/// Joins a worker thread before the application that owns it is destroyed.
class JoinOnDestroy
{
  public:
    ~JoinOnDestroy()
    {
        this->join();
    }

    void start(std::thread&& thread)
    {
        this->thread_ = std::move(thread);
    }

    void join()
    {
        if (this->thread_.joinable()) {
            this->thread_.join();
        }
    }

  private:
    std::thread thread_;
};

struct StopAfter
{
    hypha::Node*         node;
    std::chrono::seconds delay;

    void operator()() const
    {
        std::this_thread::sleep_for(delay);
        hypha::requestStop();
    }
};

/// Stops the node from a thread after a delay. The thread is joined before the
/// node it refers to is destroyed.
class Stopper
{
  public:
    void arm(hypha::Node& node, std::chrono::seconds delay = std::chrono::seconds(1))
    {
        this->thread_.start(std::thread(StopAfter{&node, delay}));
    }

  private:
    JoinOnDestroy thread_;
};

/// The runner commits the cycle after setup(). The application has no start().
class CycleApp final : public hypha::CycleNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->setRate(hypha::Hz(100.0));
        this->stopper_.arm(node);
    }

    void step(const hypha::CycleTime&) override
    {
        ++steps;
    }

    static inline int steps = 0;

  private:
    Stopper stopper_;
};

/// A plain application node with no cycle runs until it is stopped.
class TimerApp final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->timer_ =
            node.timer(hypha::Duration::fromMillis(10), hypha::bind<&TimerApp::tick>(*this));
        this->stopper_.arm(node);
    }

    void tick()
    {
        ++ticks;
    }

    static inline int ticks = 0;

  private:
    Stopper      stopper_;
    hypha::Timer timer_;
};

class SetupFailsApp final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node&) override
    {
        throw std::runtime_error("setup refused");
    }
};

/// Reports a failure from a worker thread that the application owns. The worker
/// reports at once. The stop is a hang guard that the run should never reach.
class ThreadFailApp final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->worker_.start(std::thread(FailNow{this}));
        this->stopper_.arm(node, std::chrono::seconds(5));
    }

  private:
    struct FailNow
    {
        ThreadFailApp* app;

        void operator()() const
        {
            app->failRun("control group stopped");
        }
    };

    Stopper       stopper_;
    JoinOnDestroy worker_;
};

/// Reports a failure from a worker thread while the application is destroyed.
/// The run ends cleanly first, so only the late failure can make runNode fail.
class LateFailApp final : public hypha::ApplicationNode
{
  public:
    ~LateFailApp() override
    {
        this->destroying_.store(true);
        this->worker_.join();
    }

    void setup(hypha::Node& node) override
    {
        this->worker_.start(std::thread(FailWhenDestroying{this}));
        this->stopper_.arm(node);
    }

  private:
    struct FailWhenDestroying
    {
        LateFailApp* app;

        void operator()() const
        {
            while (!app->destroying_.load()) {
                std::this_thread::sleep_for(std::chrono::milliseconds(5));
            }
            app->failRun("late failure");
        }
    };

    std::atomic<bool> destroying_{false};
    Stopper           stopper_;
    JoinOnDestroy     worker_;
};

/// Reports a failure from a timer callback on the run thread.
class TimerFailApp final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->timer_ =
            node.timer(hypha::Duration::fromMillis(10), hypha::bind<&TimerFailApp::tick>(*this));
        this->stopper_.arm(node);
    }

    void tick()
    {
        this->failRun("timer check failed");
    }

  private:
    Stopper      stopper_;
    hypha::Timer timer_;
};

/// Refuses start-up from setup() of a cycle node. The commit must not run, so
/// step() is never called. The stop is a hang guard that the run should never reach.
class SetupRefusesCycleApp final : public hypha::CycleNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->setRate(hypha::Hz(100.0));
        this->stopper_.arm(node);
        this->failRun("setup refused the cycle");
    }

    void step(const hypha::CycleTime&) override
    {
        ++steps;
    }

    static inline int steps = 0;

  private:
    Stopper stopper_;
};

/// Reports two failures in a row. Only the first reason may be logged.
class RepeatedFailureApp final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node&) override
    {
        this->failRun("first reason");
        this->failRun("second reason");
    }
};

/// Reads a key that no parameter file sets, in setup().
class RequiredKeyInSetupApp final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node&) override
    {
        static_cast<void>(hypha::config().required<int>("k8.absent"));
    }
};

/// Reads a key that no parameter file sets, in its constructor.
class RequiredKeyInConstructorApp final : public hypha::ApplicationNode
{
  public:
    RequiredKeyInConstructorApp()
    {
        static_cast<void>(hypha::config().required<int>("k8.absent"));
    }
};

/// A policy that no type of this file maps to.
inline constexpr hypha::Qos K8_OTHER_QOS{
    .name = "k8_other", .publishers = 3, .subscribers = 3, .maxDepth = 3
};

/// Opens one topic twice, with two policies: the second endpoint is refused.
class QosMismatchApp final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->first_ = node.advertise<std_msgs::Float64>(hypha::Topic{"/k8_qos_mismatch"});
        this->second_ =
            node.advertise<std_msgs::Float64>(hypha::Topic{"/k8_qos_mismatch"}, K8_OTHER_QOS);
    }

  private:
    hypha::Publisher<std_msgs::Float64> first_;
    hypha::Publisher<std_msgs::Float64> second_;
};

/// Subscribes to a port that has no table in the parameter file.
class MissingPortTableApp final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->input_ = node.subscribe<std_msgs::Float64>("k8_unwired");
    }

  private:
    hypha::Subscriber<std_msgs::Float64> input_;
};

/// A cycle port that declares another policy than the topic its setup() opened.
class CyclePortQosMismatchApp final : public hypha::CycleNode
{
  public:
    Latest<std_msgs::Float64> other{*this, hypha::Topic{"/k8_cycle_qos_mismatch"}, K8_OTHER_QOS};

    void setup(hypha::Node& node) override
    {
        this->setRate(hypha::Hz(100.0));
        this->publisher_ =
            node.advertise<std_msgs::Float64>(hypha::Topic{"/k8_cycle_qos_mismatch"});
    }

    void step(const hypha::CycleTime&) override
    {
    }

  private:
    hypha::Publisher<std_msgs::Float64> publisher_;
};

/// Its step throws, which fails the run. The stop is a hang guard.
class StepThrowsApp final : public hypha::CycleNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->setRate(hypha::Hz(100.0));
        this->stopper_.arm(node, std::chrono::seconds(2));
    }

    void step(const hypha::CycleTime&) override
    {
        throw std::runtime_error("step refused");
    }

  private:
    Stopper stopper_;
};

/// A required input whose table sets no silence_ms.
class MissingSilenceApp final : public hypha::CycleNode
{
  public:
    Latest<std_msgs::Float64> speed{*this, "speed"};

    void setup(hypha::Node&) override
    {
        this->setRate(hypha::Hz(100.0));
    }

    void step(const hypha::CycleTime&) override
    {
    }
};

/// A queued input whose perStep, fixed in code, is above the maxDepth (32) of its policy.
class ConstantPerStepApp final : public hypha::CycleNode
{
  public:
    Arrivals<std_msgs::Float64> samples{*this, hypha::Topic{"/k8_per_step"}, perStep(33)};

    void setup(hypha::Node&) override
    {
        this->setRate(hypha::Hz(100.0));
    }

    void step(const hypha::CycleTime&) override
    {
    }
};

/// A queued input whose perStep is the `queue_size` of its port table, passed on as read:
/// the framework cannot tell that the number came from the file.
class FilePerStepApp final : public hypha::CycleNode
{
  public:
    Arrivals<std_msgs::Float64> imu{
        *this, "imu", perStep(hypha::config("ports.inputs.imu").required<uint32_t>("queue_size"))
    };

    void setup(hypha::Node&) override
    {
        this->setRate(hypha::Hz(100.0));
    }

    void step(const hypha::CycleTime&) override
    {
    }
};

/// The same, with the port's own range check: the way to refuse a file value with 78.
class CheckedFilePerStepApp final : public hypha::CycleNode
{
  public:
    Arrivals<std_msgs::Float64> imu{
        *this,
        "imu",
        perStep(
            hypha::config("ports.inputs.imu")
                .required<uint32_t>("queue_size", hypha::constraints::range(1, 32))
        )
    };

    void setup(hypha::Node&) override
    {
        this->setRate(hypha::Hz(100.0));
    }

    void step(const hypha::CycleTime&) override
    {
    }
};

/// Sends from setup(): no step runs there, so the send throws.
class SendsInSetupApp final : public hypha::CycleNode
{
  public:
    Output<std_msgs::Float64> status{*this, "status"};

    void setup(hypha::Node&) override
    {
        this->setRate(hypha::Hz(100.0));
        this->status.send(std_msgs::Float64{1.0});
    }

    void step(const hypha::CycleTime&) override
    {
    }
};

/// Never constructed: runNode refuses the arguments first.
class UnreachableApp final : public hypha::ApplicationNode
{
  public:
    UnreachableApp()
    {
        constructed = true;
    }

    static inline bool constructed{false};
};

int countOccurrences(const std::string& output, const std::string& text)
{
    int         count = 0;
    std::size_t position = output.find(text);
    while (position != std::string::npos) {
        ++count;
        position = output.find(text, position + text.size());
    }
    return count;
}

/// Other tests in this binary leave a simulated clock installed. The runner
/// tests wait on real time, so they restore the monotonic clock first.
int runWithoutArguments(int (*runner)(int, char**, std::string))
{
    EXPECT_TRUE(hypha::disableSimTime());
    char  program[] = "prog";
    char* argv[] = {program, nullptr};
    return runner(1, argv, "runner_app");
}

/// Runs @p runner with the parameter file @p path, with the monotonic clock.
int runWithFile(int (*runner)(int, char**, std::string), const std::filesystem::path& path)
{
    EXPECT_TRUE(hypha::disableSimTime());
    char        program[] = "prog";
    std::string pathText = path.string();
    char*       argv[] = {program, pathText.data(), nullptr};
    return runner(2, argv, "runner_exit_code");
}

std::filesystem::path parameterFile(const char* name, const std::string& body)
{
    const std::filesystem::path path = std::filesystem::temp_directory_path() / name;
    std::ofstream               file(path);
    file << body;
    return path;
}

}  // namespace

TEST(RunNodeLifecycle, ExitCodesHaveTheirDocumentedValues)
{
    EXPECT_EQ(hypha::kExitClean, 0);
    EXPECT_EQ(hypha::kExitUsage, 2);
    EXPECT_EQ(hypha::kExitRuntime, 70);
    EXPECT_EQ(hypha::kExitRewind, 75);
    EXPECT_EQ(hypha::kExitConfiguration, 78);
}

TEST(RunNodeLifecycle, AMissingParameterFileReturnsTheConfigurationCode)
{
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_k8_no_such_file.toml";
    std::filesystem::remove(path);
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithFile(&hypha::runNode<TimerApp>, path);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(output.find("node failed in configuration loading"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AnInvalidEnvironmentReferenceReturnsTheConfigurationCode)
{
    ASSERT_EQ(::unsetenv("HYPHA_K8_UNSET_REFERENCE"), 0);
    const std::filesystem::path path =
        parameterFile("hypha_k8_unset_reference.toml", "frame = \"${HYPHA_K8_UNSET_REFERENCE}\"\n");
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithFile(&hypha::runNode<TimerApp>, path);
    const std::string output = ::testing::internal::GetCapturedStderr();
    std::filesystem::remove(path);
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(output.find("node failed in configuration loading"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AnInvalidLogLevelReturnsTheConfigurationCode)
{
    const std::filesystem::path path =
        parameterFile("hypha_k8_log_level.toml", "[log]\nlevel = \"loud\"\n");
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithFile(&hypha::runNode<TimerApp>, path);
    const std::string output = ::testing::internal::GetCapturedStderr();
    std::filesystem::remove(path);
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(output.find("[log] level: unknown level"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AnInvalidNodeNameInTheParameterFileReturnsTheConfigurationCode)
{
    const std::filesystem::path path =
        parameterFile("hypha_k8_node_name.toml", "[node]\nname = \"Bad Name\"\n");
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithFile(&hypha::runNode<TimerApp>, path);
    const std::string output = ::testing::internal::GetCapturedStderr();
    std::filesystem::remove(path);
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(output.find("'Bad Name' is not a node name"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AnInvalidNodeNameInTheCodeReturnsTheRuntimeCode)
{
    EXPECT_TRUE(hypha::disableSimTime());
    char  program[] = "prog";
    char* argv[] = {program, nullptr};
    ::testing::internal::CaptureStderr();
    const int         exitCode = hypha::runNode<TimerApp>(1, argv, "Bad Name");
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitRuntime);
    EXPECT_NE(output.find("'Bad Name' is not a node name"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AParameterErrorInSetupReturnsTheConfigurationCode)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<RequiredKeyInSetupApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(output.find("node failed in setup"), std::string::npos) << output;
    EXPECT_NE(output.find("k8.absent"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AParameterErrorInTheConstructorReturnsTheConfigurationCode)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<RequiredKeyInConstructorApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(output.find("node failed in application construction"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AQosMismatchReturnsTheConfigurationCode)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<QosMismatchApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(output.find("QosMismatch"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AMissingPortTableReturnsTheConfigurationCode)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<MissingPortTableApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(output.find("ports.inputs.k8_unwired"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AQosMismatchOfACyclePortReturnsTheConfigurationCode)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<CyclePortQosMismatchApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(output.find("node failed in cycle start"), std::string::npos) << output;
    EXPECT_NE(output.find("QosMismatch"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AFailureInAStepReturnsTheRuntimeCode)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<StepThrowsApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitRuntime);
    EXPECT_NE(output.find("node run failed"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AUsageErrorReturnsTwo)
{
    UnreachableApp::constructed = false;
    char  program[] = "prog";
    char  first[] = "a.toml";
    char  second[] = "b.toml";
    char* argv[] = {program, first, second, nullptr};
    ::testing::internal::CaptureStderr();
    const int         exitCode = hypha::runNode<UnreachableApp>(3, argv, "runner_usage");
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitUsage);
    EXPECT_FALSE(UnreachableApp::constructed);
    EXPECT_NE(output.find("usage: runner_usage [parameter_file]"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, RunnerCommitsTheCycleAfterSetup)
{
    CycleApp::steps = 0;
    EXPECT_EQ(runWithoutArguments(&hypha::runNode<CycleApp>), hypha::kExitClean);
    EXPECT_GT(CycleApp::steps, 0);
}

TEST(RunNodeLifecycle, ApplicationNodeRunsWithoutACycle)
{
    TimerApp::ticks = 0;
    EXPECT_EQ(runWithoutArguments(&hypha::runNode<TimerApp>), 0);
    EXPECT_GT(TimerApp::ticks, 0);
}

TEST(RunNodeLifecycle, FailRunFromAnotherThreadReturnsTheRuntimeCode)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<ThreadFailApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitRuntime);
    EXPECT_EQ(countOccurrences(output, "application failed: control group stopped"), 1);
}

TEST(RunNodeLifecycle, FailRunFromATimerReturnsTheRuntimeCode)
{
    EXPECT_EQ(runWithoutArguments(&hypha::runNode<TimerFailApp>), hypha::kExitRuntime);
}

TEST(RunNodeLifecycle, FailRunFromSetupOfACycleNodeSkipsTheCommit)
{
    SetupRefusesCycleApp::steps = 0;
    EXPECT_EQ(runWithoutArguments(&hypha::runNode<SetupRefusesCycleApp>), hypha::kExitRuntime);
    EXPECT_EQ(SetupRefusesCycleApp::steps, 0);
}

TEST(RunNodeLifecycle, FailRunDuringApplicationDestructionReturnsTheRuntimeCode)
{
    EXPECT_EQ(runWithoutArguments(&hypha::runNode<LateFailApp>), hypha::kExitRuntime);
}

TEST(RunNodeLifecycle, SecondFailureReasonIsNotLogged)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<RepeatedFailureApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitRuntime);
    EXPECT_EQ(countOccurrences(output, "application failed: first reason"), 1);
    EXPECT_EQ(countOccurrences(output, "second reason"), 0);
}

TEST(RunNodeLifecycle, SetupFailureLogsTheSetupPhase)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<SetupFailsApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitRuntime);
    EXPECT_NE(output.find("setup: setup refused"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AnUnreadKeyReturnsTheConfigurationCode)
{
    EXPECT_TRUE(hypha::disableSimTime());
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_run_node_unread.toml";
    {
        std::ofstream file(path);
        file << "typo_parameter = 42\n";
    }
    CycleApp::steps = 0;
    char        program[] = "prog";
    std::string pathText = path.string();
    char*       argv[] = {program, pathText.data(), nullptr};
    ::testing::internal::CaptureStderr();
    const int         exitCode = hypha::runNode<CycleApp>(2, argv, "runner_unread");
    const std::string output = ::testing::internal::GetCapturedStderr();
    std::filesystem::remove(path);
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_EQ(CycleApp::steps, 0);
    EXPECT_NE(output.find("typo_parameter"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, AMissingSilenceLimitReturnsTheConfigurationCode)
{
    const std::filesystem::path path = parameterFile(
        "hypha_run_node_missing_silence.toml", "[ports.inputs.speed]\ntopic='/d1_speed'\n"
    );
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithFile(&hypha::runNode<MissingSilenceApp>, path);
    const std::string output = ::testing::internal::GetCapturedStderr();
    std::filesystem::remove(path);
    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(
        output.find(
            "input 'speed': missing key 'silence_ms': a required input needs a limit, or "
            "the tag hypha::unsupervised: NotAvailable"
        ),
        std::string::npos
    ) << output;
}

TEST(RunNodeLifecycle, APerStepAboveTheLimitFixedInCodeReturnsTheRuntimeCodeAndNamesTheLimit)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<ConstantPerStepApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitRuntime) << output;
    EXPECT_NE(output.find("node failed in application construction"), std::string::npos) << output;
    EXPECT_NE(
        output.find(
            "a queue of 33 samples per step (perStep) exceeds the maxDepth of its QoS policy "
            "'default' (publishers 8, subscribers 16, maxDepth 32)"
        ),
        std::string::npos
    ) << output;
}

TEST(RunNodeLifecycle, APerStepAboveTheLimitPassedOnFromTheFileStillReturnsTheRuntimeCode)
{
    const std::filesystem::path path = parameterFile(
        "hypha_run_node_per_step_file.toml",
        "[ports.inputs.imu]\ntopic = '/k8_per_step_file'\nqueue_size = 33\nsilence_ms = 100\n"
    );
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithFile(&hypha::runNode<FilePerStepApp>, path);
    const std::string output = ::testing::internal::GetCapturedStderr();
    std::filesystem::remove(path);
    EXPECT_EQ(exitCode, hypha::kExitRuntime) << output;
    EXPECT_NE(output.find("maxDepth 32"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, ARangeCheckOfAPerStepReadFromTheFileReturnsTheConfigurationCode)
{
    const std::filesystem::path path = parameterFile(
        "hypha_run_node_per_step_checked.toml",
        "[ports.inputs.imu]\ntopic = '/k8_per_step_checked'\nqueue_size = 33\nsilence_ms = 100\n"
    );
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithFile(&hypha::runNode<CheckedFilePerStepApp>, path);
    const std::string output = ::testing::internal::GetCapturedStderr();
    std::filesystem::remove(path);
    EXPECT_EQ(exitCode, hypha::kExitConfiguration) << output;
    EXPECT_NE(output.find("queue_size"), std::string::npos) << output;
}

TEST(RunNodeLifecycle, ASetupThatSendsFailsTheNodeNamingTheOutput)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = runWithoutArguments(&hypha::runNode<SendsInSetupApp>);
    const std::string output = ::testing::internal::GetCapturedStderr();
    EXPECT_EQ(exitCode, hypha::kExitRuntime);
    EXPECT_NE(
        output.find("node failed in setup: hypha: output 'status': send() outside a step"),
        std::string::npos
    ) << output;
}
