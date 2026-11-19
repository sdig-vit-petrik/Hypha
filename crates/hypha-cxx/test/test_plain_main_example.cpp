// The custom main of a plain hypha::Node, as docs/ros1-migration-guide.md section 10.9
// shows it: every way out maps to the exit code a supervisor expects. Keep the function
// below equal to the example of the guide.

#include "hypha/application_node.h"
#include "hypha/config.h"
#include "hypha/hypha.h"

#include <gtest/gtest.h>

#include <exception>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <vector>

namespace {

/// Ends the run from its first timer call, so the test's run returns.
struct StopTheRun
{
    void operator()() const
    {
        hypha::requestStop();
    }
};

/// What the node reads and owns: one checked parameter and one timer.
class Locator
{
  public:
    explicit Locator(hypha::Node& node)
        : rateHz_(hypha::config().required<double>("rate_hz", hypha::constraints::positive()))
        , timer_(node.timer(hypha::Duration::fromMillis(1), StopTheRun{}))
    {
    }

  private:
    double       rateHz_;
    hypha::Timer timer_;
};

/// The exit code of a run that ended: 0, 75 after simulated time went back, else 70.
int exitCodeOf(const hypha::RunResult& result)
{
    const bool rewound = result.status() == hypha::Status::ClockRewound;
    if (rewound) {
        return hypha::kExitRewind;
    }
    return result.ok() ? hypha::kExitClean : hypha::kExitRuntime;
}

int plainMain(int argc, char** argv)
{
    const bool tooManyArguments = argc > 2;
    if (tooManyArguments) {
        std::cerr << "usage: locator [parameter_file]\n";
        return hypha::kExitUsage;
    }
    const std::string parameterFile = argc == 2 ? std::string(argv[1]) : std::string{};
    try {
        // Loaded first and alone: a file that is missing or does not parse is a
        // configuration fault, which a restart cannot fix.
        const hypha::ConfigContext configuration(parameterFile);
        try {
            hypha::Node                    node("locator", parameterFile);
            const Locator                  locator(node);
            const std::vector<std::string> unread = hypha::config().unreadKeys();
            if (!unread.empty()) {
                std::cerr << "unread parameter(s): " << unread.front() << '\n';
                return hypha::kExitConfiguration;
            }
            return exitCodeOf(node.run());
        } catch (const hypha::ConfigError& error) {
            std::cerr << "configuration refused: " << error.what() << '\n';
            return hypha::kExitConfiguration;
        } catch (const std::exception& error) {
            std::cerr << "start-up failed: " << error.what() << '\n';
            return hypha::kExitRuntime;
        }
    } catch (const std::exception& error) {
        std::cerr << "parameter file refused: " << error.what() << '\n';
        return hypha::kExitConfiguration;
    }
}

/// Runs plainMain with a parameter file of @p text.
int runWith(const std::string& text)
{
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_plain_main_example.toml";
    {
        std::ofstream output(path);
        output << text;
    }
    std::string program = "locator";
    std::string file = path.string();
    char*       argv[] = {program.data(), file.data()};
    const int   code = plainMain(2, argv);
    std::filesystem::remove(path);
    return code;
}

}  // namespace

TEST(PlainMainExample, EveryWayOutHasTheExitCodeASupervisorExpects)
{
    std::string program = "locator";
    std::string first = "a.toml";
    std::string second = "b.toml";
    char*       tooMany[] = {program.data(), first.data(), second.data()};
    EXPECT_EQ(plainMain(3, tooMany), hypha::kExitUsage);

    std::string missing = "/nonexistent/hypha_plain_main_example.toml";
    char*       missingFile[] = {program.data(), missing.data()};
    EXPECT_EQ(plainMain(2, missingFile), hypha::kExitConfiguration);

    EXPECT_EQ(runWith("rate_hz = 'fast'\n"), hypha::kExitConfiguration);
    EXPECT_EQ(runWith("rate_hz = -1.0\n"), hypha::kExitConfiguration);
    EXPECT_EQ(runWith("rate_hz = 10.0\ntypo = 1\n"), hypha::kExitConfiguration);

    // The timer of the test's Locator requests the stop: a run that ended as requested.
    EXPECT_EQ(runWith("rate_hz = 10.0\n"), hypha::kExitClean);
}

TEST(PlainMainExample, ARewoundRunExitsWithTheRewindCode)
{
    const hypha::RunResult rewound = hypha::RunResult::apiFailure(hypha::Status::ClockRewound);
    EXPECT_EQ(exitCodeOf(rewound), hypha::kExitRewind);
    const hypha::RunResult failed = hypha::RunResult::apiFailure(hypha::Status::InvalidState);
    EXPECT_EQ(exitCodeOf(failed), hypha::kExitRuntime);
}
