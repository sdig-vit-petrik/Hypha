#pragma once

#include "hypha/config.h"
#include "hypha/detail/report.h"
#include "hypha/introspection.h"
#include "hypha/node.h"

#include <expected>
#include <iostream>
#include <mutex>
#include <optional>
#include <string>
#include <string_view>
#include <type_traits>
#include <vector>

namespace hypha {

// The exit codes of runNode(). A supervisor restarts on kExitRuntime and never on
// kExitConfiguration.

/// The run ended with a clean stop.
inline constexpr int kExitClean = 0;
/// The arguments are wrong: more than one parameter file.
inline constexpr int kExitUsage = 2;
/// The run failed, or start-up failed for a reason other than the configuration.
inline constexpr int kExitRuntime = 70;
/// Simulated time went back (a backward seek): hypha launch restarts the unit at once.
inline constexpr int kExitRewind = 75;
/// The configuration was refused before the run: a restart cannot fix it.
inline constexpr int kExitConfiguration = 78;

namespace detail {

/// The phases of runNode() that exitCodeFor() reads.
inline constexpr const char* kPhaseConfigurationLoading = "configuration loading";
inline constexpr const char* kPhaseConfigurationCheck = "configuration check";
inline constexpr const char* kPhaseRun = "run";

/// @brief What ended a start-up or a run that failed.
enum class Failure {
    /// A hypha::ConfigError, ParameterError among them, escaped a phase.
    ConfigError,
    /// Any other exception escaped a phase.
    OtherException,
    /// failRun() was called, from any thread and in any phase.
    FailRun,
    /// run() returned a result that is not OK.
    RunResult,
};

/// @brief The exit code of runNode() for @p failure in @p phase.
inline int exitCodeFor(std::string_view phase, Failure failure)
{
    if (failure == Failure::FailRun) {
        return kExitRuntime;
    }
    if (failure == Failure::RunResult) {
        return kExitRuntime;
    }
    if (phase == kPhaseRun) {
        return kExitRuntime;
    }
    if (failure == Failure::ConfigError) {
        return kExitConfiguration;
    }
    const bool configurationPhase =
        phase == kPhaseConfigurationLoading || phase == kPhaseConfigurationCheck;
    if (configurationPhase) {
        return kExitConfiguration;
    }
    return kExitRuntime;
}

/// @brief The first failure an application reports. Thread-safe.
class RunFailure
{
  public:
    /// @return True only for the first reason. Later reasons are dropped.
    bool record(const std::string& reason)
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        if (this->reason_.has_value()) {
            return false;
        }
        this->reason_ = reason;
        return true;
    }

    bool failed() const
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        return this->reason_.has_value();
    }

  private:
    mutable std::mutex         mutex_;
    std::optional<std::string> reason_;
};

/// @return The fields of a run outcome, as numbers, for one log line.
inline std::string describeRun(const RunResult& result)
{
    return "call status " + std::string(result.call().name()) + ", stop reason "
         + std::to_string(static_cast<int>(result.reason())) + ", run status "
         + std::string(result.status().name()) + ", failed attachment "
         + std::to_string(result.failedAttachment());
}

/// @brief Publishes the node manifest (latched, `~/introspection`) and the node state
///        (`~/state`, every second) of an application.
class NodeReport
{
  public:
    /// @brief Open both publishers and the timer, and publish.
    /// @throws std::runtime_error when an endpoint or a snapshot is refused.
    explicit NodeReport(Node& node)
        : manifest_(
              node.advertise<hypha_msgs::NodeIntrospection>(Topic{"~/introspection"}, latched)
          )
        , state_(node.advertise<hypha_msgs::NodeState>(Topic{"~/state"}))
        , timer_(node.timer(Duration::fromSeconds(1.0), Tick{this}))
    {
        // The publishers above are rows of the manifest, so the snapshot comes last.
        const bool manifestSent = this->manifest_.publish(require(introspect(), "node manifest"));
        if (!manifestSent) {
            require(this->manifest_.lastPublishStatus(), "node manifest");
        }
        this->publishState();
    }

    NodeReport(const NodeReport&) = delete;
    NodeReport& operator=(const NodeReport&) = delete;
    NodeReport(NodeReport&&) = delete;
    NodeReport& operator=(NodeReport&&) = delete;

  private:
    /// The timer keeps the callable, so it is a named functor that borrows the report.
    struct Tick
    {
        NodeReport* report;

        void operator()() const
        {
            this->report->publishState();
        }
    };

    void publishState()
    {
        const std::expected<hypha_msgs::NodeState, Status> state = nodeState();
        if (state) {
            static_cast<void>(this->state_.publish(*state));
        }
    }

    Publisher<hypha_msgs::NodeIntrospection> manifest_;
    Publisher<hypha_msgs::NodeState>         state_;
    Timer                                    timer_;
};

}  // namespace detail

/// @brief An application: the object runNode() constructs, sets up and runs.
///
/// The constructor may read configuration but has no bus yet; setup() creates the endpoints.
class ApplicationNode
{
  public:
    virtual ~ApplicationNode() = default;

    /// @brief Create endpoints and finish the configuration of the node.
    /// Throw, or call failRun(), to refuse start-up.
    virtual void setup(Node&)
    {
    }

    /// @brief Called once after start-up, before the run, when every endpoint and cycle exists.
    /// Throw, or call failRun(), to refuse the run.
    virtual void started(Node&)
    {
    }

  protected:
    /// @brief Report a fatal application failure and end the run as failed. Thread-safe.
    ///
    /// The first reason makes runNode return kExitRuntime; later calls do nothing. A cycle
    /// step that returns after the call sends none of its messages. Call it from setup() or
    /// later, including during destruction; join the application's threads in its destructor.
    void failRun(const std::string& reason)
    {
        const bool firstFailure = this->failure_ == nullptr || this->failure_->record(reason);
        if (!firstFailure) {
            return;
        }
        detail::error("application failed: " + reason);
        backend::runFail();
    }

  private:
    template <typename App, typename... Args>
    friend int runNode(int argc, char** argv, std::string nodeName, Args&&... args);

    virtual std::expected<void, std::string> commit(Node&)
    {
        return {};
    }

    /// The steps after the application object exists; @p phase names the current one.
    /// @return The exit code of runNode() for the run. An exception goes to the caller.
    static int runLifecycle(
        Node& node, ApplicationNode& lifecycle, detail::RunFailure& failure, const char*& phase
    )
    {
        phase = "setup";
        lifecycle.setup(node);
        if (failure.failed()) {
            return detail::exitCodeFor(phase, detail::Failure::FailRun);
        }
        phase = "cycle start";
        const std::expected<void, std::string> committed = lifecycle.commit(node);
        if (!committed) {
            throw std::runtime_error(committed.error());
        }
        phase = detail::kPhaseConfigurationCheck;
        const std::vector<std::string> unread = config().unreadKeys();
        if (!unread.empty()) {
            std::string joined;
            for (const std::string& key : unread) {
                joined += (joined.empty() ? "" : ", ") + key;
            }
            throw std::runtime_error("unread parameter(s): " + joined);
        }
        phase = "node report";
        const detail::NodeReport report(node);
        phase = "started";
        lifecycle.started(node);
        if (failure.failed()) {
            return detail::exitCodeFor(phase, detail::Failure::FailRun);
        }
        // A stop requested during start-up is kept: the run then returns at once.
        phase = detail::kPhaseRun;
        const RunResult result = node.run();
        const bool      rewound = result.status() == Status::ClockRewound;
        if (rewound) {
            detail::error(
                "simulated time went back; exiting 75 so hypha launch restarts this unit"
            );
            return kExitRewind;
        }
        if (!result.ok()) {
            detail::error("node run failed: " + detail::describeRun(result));
            return detail::exitCodeFor(phase, detail::Failure::RunResult);
        }
        return kExitClean;
    }

    detail::RunFailure* failure_ = nullptr;
};

/// @brief Run an application node: load the parameter file, construct the application
///        from @p args, join the bus, set up, and run until the node stops.
///
/// `argv` holds zero or one argument, the parameter file.
/// @return One of the kExit codes above.
template <typename App, typename... Args>
int runNode(int argc, char** argv, std::string nodeName, Args&&... args)
{
    static_assert(std::is_base_of_v<ApplicationNode, App>);
    std::string abiDiagnostic;
    if (!backend::verifyAbi(&abiDiagnostic)) {
        // A restart cannot fix a library of another revision.
        std::cerr << abiDiagnostic << '\n';
        return kExitConfiguration;
    }
    const bool tooManyArguments = argc > 2;
    if (tooManyArguments) {
        std::cerr << "usage: " << nodeName << " [parameter_file]\n";
        return kExitUsage;
    }
    const std::string parameterFile = argc == 2 ? std::string(argv[1]) : std::string{};

    // Declared before Node and App: it outlives every thread they own.
    detail::RunFailure failure;
    int                exitCode = kExitRuntime;
    const char*        phase = detail::kPhaseConfigurationLoading;
    try {
        ConfigContext configContext(parameterFile);
        // Declared before the application: its endpoints and ports must not outlive the node.
        std::optional<Node> node;
        phase = "application construction";
        App              application(std::forward<Args>(args)...);
        ApplicationNode& lifecycle = application;
        lifecycle.failure_ = &failure;
        phase = "node creation";
        node.emplace(nodeName, parameterFile);
        exitCode = ApplicationNode::runLifecycle(*node, lifecycle, failure, phase);
    } catch (const ConfigError& error) {
        detail::error("node failed in " + std::string(phase) + ": " + std::string(error.what()));
        exitCode = detail::exitCodeFor(phase, detail::Failure::ConfigError);
    } catch (const std::exception& error) {
        detail::error("node failed in " + std::string(phase) + ": " + std::string(error.what()));
        exitCode = detail::exitCodeFor(phase, detail::Failure::OtherException);
    } catch (...) {
        detail::error("node failed in " + std::string(phase) + ": unknown exception");
        exitCode = detail::exitCodeFor(phase, detail::Failure::OtherException);
    }
    // Read after the application is destroyed, to catch a failRun() from its destructor.
    if (failure.failed()) {
        exitCode = detail::exitCodeFor(phase, detail::Failure::FailRun);
    }
    return exitCode;
}

}  // namespace hypha
