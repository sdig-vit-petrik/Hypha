#include "hypha/hypha.h"
#include <hypha_msgs/hypha_msgs/NodeIntrospection.h>
#include <hypha_msgs/hypha_msgs/NodeState.h>

#include "introspection_text.h"

#include <gtest/gtest.h>

#include <algorithm>
#include <filesystem>
#include <fstream>
#include <functional>
#include <optional>
#include <sstream>
#include <stdexcept>
#include <string>

namespace {

std::filesystem::path configFile(const std::string& body)
{
    const auto path = std::filesystem::temp_directory_path()
                    / ("hypha_prepare_" + std::to_string(std::hash<std::string>{}(body)) + ".toml");
    std::ofstream output(path);
    output << body;
    output.close();
    return path;
}

/// An application that refuses its configuration in the constructor.
class ConstructorFailureApp final : public hypha::ApplicationNode
{
  public:
    ConstructorFailureApp()
    {
        readValue = hypha::config().required<int>("app.early");
        throw std::runtime_error("constructor rejected");
    }

    void setup(hypha::Node&) override
    {
        setupCalled = true;
    }

    static inline int  readValue{0};
    static inline bool setupCalled{false};
};

/// Reads one key in the constructor and one in setup(), and takes its constructor
/// arguments from runNode().
class SharedConfigApp final : public hypha::ApplicationNode
{
  public:
    SharedConfigApp(int offset, std::string label)
        : constructorValue(hypha::config().required<int>("app.early") + offset)
    {
        defaultedValue = hypha::config().optional<int>("app.edefault", 3141);
        hypha::recordConfig("build.label", label);
        seenValue = this->constructorValue;
    }

    void setup(hypha::Node&) override
    {
        static_cast<void>(hypha::config().required<int>("app.setup"));
        snapshotText = test_support::introspectionText();
        hypha::requestStop();
    }

    int                       constructorValue;
    static inline int         seenValue{0};
    static inline int         defaultedValue{0};
    static inline std::string snapshotText;
};

template <typename App, typename... Args>
int runWithConfig(const std::filesystem::path& file, const char* name, Args&&... args)
{
    std::string path = file.string();
    char*       arguments[] = {const_cast<char*>(name), path.data()};
    return hypha::runNode<App>(2, arguments, name, std::forward<Args>(args)...);
}

std::string rowContaining(const std::string& text, const std::string& key)
{
    std::istringstream lines(text);
    std::string        line;
    while (std::getline(lines, line)) {
        if (line.find(key) != std::string::npos) {
            return line;
        }
    }
    return {};
}

bool nodeIsOnTheBus(const std::string& name)
{
    const auto nodes = hypha::listNodes();
    return std::any_of(nodes.begin(), nodes.end(), [&name](const auto& node) {
        return node.name == name;
    });
}

TEST(ApplicationConstruction, AFailureHappensBeforeTheNodeJoinsTheBus)
{
    const std::filesystem::path file = configFile("[app]\nearly = 17\n");
    ConstructorFailureApp::readValue = 0;
    ConstructorFailureApp::setupCalled = false;
    testing::internal::CaptureStderr();

    const int result = runWithConfig<ConstructorFailureApp>(file, "constructor_failure_no_bus");

    const std::string errors = testing::internal::GetCapturedStderr();
    std::filesystem::remove(file);
    // A generic exception is a runtime failure, even in the constructor.
    EXPECT_EQ(result, hypha::kExitRuntime);
    EXPECT_EQ(ConstructorFailureApp::readValue, 17);
    EXPECT_FALSE(ConstructorFailureApp::setupCalled);
    EXPECT_FALSE(nodeIsOnTheBus("constructor_failure_no_bus"));
    EXPECT_NE(errors.find("application construction: constructor rejected"), std::string::npos)
        << errors;
}

TEST(ApplicationConstruction, RunNodeForwardsItsArgumentsAndOneConfigServesEveryPhase)
{
    const std::filesystem::path file = configFile("[app]\nearly = 4\nsetup = 9\n");
    SharedConfigApp::snapshotText.clear();

    const int result = runWithConfig<SharedConfigApp>(file, "shared_config", 38, "v1.2.3");

    std::filesystem::remove(file);
    EXPECT_EQ(result, 0);
    EXPECT_EQ(SharedConfigApp::seenValue, 42);
    const std::string& text = SharedConfigApp::snapshotText;
    // A read before the node exists is a row of the node's snapshot all the same.
    EXPECT_NE(rowContaining(text, "app.early").find("4"), std::string::npos) << text;
    const std::string defaultRow = rowContaining(text, "app.edefault");
    EXPECT_NE(defaultRow.find("3141"), std::string::npos) << text;
    EXPECT_NE(defaultRow.find("default"), std::string::npos) << text;
    const std::string buildRow = rowContaining(text, "build.label");
    EXPECT_NE(buildRow.find("v1.2.3"), std::string::npos) << text;
    EXPECT_NE(buildRow.find("application"), std::string::npos) << text;
}

TEST(ApplicationConstruction, AKeyNobodyReadsReturnsTheConfigurationCode)
{
    const std::filesystem::path file = configFile("[app]\nearly = 4\nsetup = 9\nnever_read = 1\n");
    testing::internal::CaptureStderr();

    const int result = runWithConfig<SharedConfigApp>(file, "unread_key", 0, "v");

    const std::string errors = testing::internal::GetCapturedStderr();
    std::filesystem::remove(file);
    EXPECT_EQ(result, hypha::kExitConfiguration);
    EXPECT_NE(errors.find("app.never_read"), std::string::npos) << errors;
    EXPECT_EQ(errors.find("app.early"), std::string::npos) << errors;
}

/// Reads its own `~/state` and `~/introspection`, and stops once both arrived.
class ReportReaderApp final : public hypha::ApplicationNode
{
  public:
    struct OnState
    {
        void operator()(const hypha::MessagePtr<hypha_msgs::NodeState>& message) const
        {
            state = *message;
            stopWhenBothArrived();
        }
    };

    struct OnManifest
    {
        void operator()(const hypha::MessagePtr<hypha_msgs::NodeIntrospection>& message) const
        {
            manifest = *message;
            stopWhenBothArrived();
        }
    };

    void setup(hypha::Node& node) override
    {
        this->output = node.advertise<hypha_msgs::NodeState>(hypha::Topic{"/report_reader/out"});
        this->stateReader =
            node.subscribe<hypha_msgs::NodeState>(hypha::Topic{"~/state"}, OnState{});
        this->manifestReader = node.subscribe<hypha_msgs::NodeIntrospection>(
            hypha::Topic{"~/introspection"}, OnManifest{}
        );
    }

    /// The manifest is complete by now: it lists the node report's own publishers.
    void started(hypha::Node&) override
    {
        const auto snapshot = hypha::introspect();
        startedPublishers = snapshot ? snapshot->publishers.size() : 0U;
    }

    static void stopWhenBothArrived()
    {
        if (state.has_value() && manifest.has_value()) {
            hypha::requestStop();
        }
    }

    hypha::Publisher<hypha_msgs::NodeState>          output;
    hypha::Subscriber<hypha_msgs::NodeState>         stateReader;
    hypha::Subscriber<hypha_msgs::NodeIntrospection> manifestReader;

    static inline size_t                                       startedPublishers = 0;
    static inline std::optional<hypha_msgs::NodeState>         state;
    static inline std::optional<hypha_msgs::NodeIntrospection> manifest;
};

TEST(ApplicationReport, RunNodePublishesTheStateAndTheManifestOfTheNode)
{
    const std::filesystem::path file = configFile("");
    ReportReaderApp::state.reset();
    ReportReaderApp::manifest.reset();
    ReportReaderApp::startedPublishers = 0;
    testing::internal::CaptureStderr();

    const int result = runWithConfig<ReportReaderApp>(file, "report_reader");

    const std::string log = testing::internal::GetCapturedStderr();
    std::filesystem::remove(file);
    ASSERT_EQ(result, 0) << log;
    ASSERT_TRUE(ReportReaderApp::state.has_value());
    EXPECT_EQ(ReportReaderApp::state->node, "report_reader");
    // No cycle input is required, so nothing is waited for.
    EXPECT_EQ(ReportReaderApp::state->state, hypha_msgs::NodeState::INPUTS_USABLE);
    EXPECT_TRUE(ReportReaderApp::state->inputs.empty());
    ASSERT_TRUE(ReportReaderApp::manifest.has_value());
    const auto& publishers = ReportReaderApp::manifest->publishers;
    const auto  publishes = [&](const std::string& topic) {
        return std::any_of(publishers.begin(), publishers.end(), [&topic](const auto& row) {
            return row.topic == topic;
        });
    };
    EXPECT_TRUE(publishes("/report_reader/out"));
    EXPECT_TRUE(publishes("/report_reader/state"));
    EXPECT_TRUE(publishes("/report_reader/introspection"));
    // started() sees the complete manifest; hypha logs no manifest text.
    EXPECT_EQ(ReportReaderApp::startedPublishers, publishers.size());
    EXPECT_EQ(log.find("node report_reader"), std::string::npos) << log;
}

}  // namespace
