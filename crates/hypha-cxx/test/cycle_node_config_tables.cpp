// Class-based cycle tables ([cycles.<name>] and [ports.inputs.<name>]) read
// through runNode, and the input role the snapshot shows for a shared source.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include <hypha_msgs/std_msgs/Float64.h>

#include "introspection_text.h"
#include "test_types.h"

#include <gtest/gtest.h>

#include <expected>
#include <filesystem>
#include <fstream>
#include <optional>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

using Float64 = std_msgs::Float64;

namespace {

std::string observedSnapshot;

struct CaptureAndStop
{
    void operator()() const
    {
        observedSnapshot = test_support::introspectionText();
        hypha::requestStop();
    }
};

class TableNode final : public hypha::CycleNode
{
  public:
    Arrivals<Float64> speed{*this, "speed", perStep(2)};

    void setup(hypha::Node& node) override
    {
        this->setRate(hypha::Hz(100.0));
        this->timer_ = node.timer(hypha::Duration::fromMillis(1), CaptureAndStop{});
        hypha_test::advance(hypha::Duration::fromMillis(2));
    }

    void step(const hypha::CycleTime&) override
    {
    }

  private:
    hypha::Timer timer_;
};

/// Takes its rate from `[cycle] rate_hz`. Its first step checks the clock of a step.
class RateFromTableNode final : public hypha::CycleNode
{
  public:
    hypha::Arrivals<Float64> speed{*this, "speed"};

    /// Moves simulated time past the first deadline of the cycle, once the run has it.
    struct PassTheDeadline
    {
        void operator()() const
        {
            hypha_test::advance(hypha::Duration::fromMillis(25));
        }
    };

    void setup(hypha::Node& node) override
    {
        this->timer_ = node.timer(hypha::Duration::fromMillis(1), PassTheDeadline{});
        hypha_test::advance(hypha::Duration::fromMillis(2));
    }

    void step(const hypha::CycleTime& time) override
    {
        observedSnapshot = test_support::introspectionText();
        const bool nowIsTheStepTime = hypha::now() == time.now;
        // The clock moves on while the step runs; the step still reads its own time.
        hypha_test::advance(hypha::Duration::fromMillis(7));
        stepClockHeld = nowIsTheStepTime && hypha::now() == time.now;
        stampIsNow = hypha::fromRosTime(time.stamp) == time.now;
        hypha::requestStop();
    }

    static inline bool stepClockHeld{false};
    static inline bool stampIsNow{false};

  private:
    hypha::Timer timer_;
};

/// A port whose name the parameter file chooses: the constructor reads configuration.
class LateBoundPortNode final : public hypha::CycleNode
{
  public:
    Latest<Float64> signal{*this, hypha::config().required<std::string>("signal_port")};

    void setup(hypha::Node& node) override
    {
        this->setRate(hypha::Hz(100.0));
        this->timer_ = node.timer(hypha::Duration::fromMillis(1), CaptureAndStop{});
        hypha_test::advance(hypha::Duration::fromMillis(2));
    }

    void step(const hypha::CycleTime&) override
    {
    }

  private:
    hypha::Timer timer_;
};

class CycleNodeConfigTables : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        observedSnapshot.clear();
    }

    void TearDown() override
    {
        std::filesystem::remove(this->path);
    }

    template <typename NodeType = TableNode>
    int run(const std::string& cycleKeys, const std::string& portKeys)
    {
        this->path = std::filesystem::temp_directory_path()
                   / ("hypha_cycle_node_config_tables_"
                      + std::to_string(std::hash<std::string>{}(cycleKeys + portKeys)) + ".toml");
        std::ofstream output(this->path);
        output << "[cycle]\n" << cycleKeys << "\n";
        // One hour: these tests are about the tables, not about staleness.
        output << "[ports.inputs.speed]\ntopic='" << hypha_test::testTopic("/speed")
               << "'\nsilence_ms = 3600000\n"
               << portKeys << "\n";
        output.close();
        char        program[] = "prog";
        std::string file = this->path.string();
        char*       arguments[] = {program, file.data(), nullptr};
        return hypha::runNode<NodeType>(2, arguments, "cycle_table_node");
    }

    std::filesystem::path path;
};

TEST_F(CycleNodeConfigTables, TheArrivalBudgetAndTheQueuedInputReachTheSnapshot)
{
    const int exitCode = this->run("arrival_budget = 3", "");

    EXPECT_EQ(exitCode, 0);
    EXPECT_NE(observedSnapshot.find("cycle.arrival_budget"), std::string::npos) << observedSnapshot;
    // A queued input always drops its oldest work, so the snapshot names no policy.
    EXPECT_NE(observedSnapshot.find("queued=2"), std::string::npos) << observedSnapshot;
}

TEST_F(CycleNodeConfigTables, TheRateComesFromTheCycleTable)
{
    RateFromTableNode::stepClockHeld = false;
    RateFromTableNode::stampIsNow = false;

    const int exitCode = this->run<RateFromTableNode>("rate_hz = 50", "");

    EXPECT_EQ(exitCode, 0);
    EXPECT_NE(observedSnapshot.find("cycle.rate_hz"), std::string::npos) << observedSnapshot;
    // The cycle has the name of its node and the period of 50 Hz.
    EXPECT_NE(observedSnapshot.find("cycle_table_node"), std::string::npos) << observedSnapshot;
    EXPECT_NE(observedSnapshot.find("50.0"), std::string::npos) << observedSnapshot;
    EXPECT_TRUE(RateFromTableNode::stepClockHeld) << "now() in a step is the step's time";
    EXPECT_TRUE(RateFromTableNode::stampIsNow);
}

TEST_F(CycleNodeConfigTables, ARateInTheTableAndInCodeReturnsTheConfigurationCode)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = this->run("rate_hz = 50", "");
    const std::string errors = ::testing::internal::GetCapturedStderr();

    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(errors.find("keep one"), std::string::npos) << errors;
}

TEST_F(CycleNodeConfigTables, ACycleNodeWithoutARateReturnsTheConfigurationCode)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = this->run<RateFromTableNode>("", "");
    const std::string errors = ::testing::internal::GetCapturedStderr();

    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(errors.find("neither a rate nor a trigger"), std::string::npos) << errors;
}

TEST_F(CycleNodeConfigTables, ARateThatIsNotPositiveReturnsTheConfigurationCode)
{
    ::testing::internal::CaptureStderr();
    const int         exitCode = this->run<RateFromTableNode>("rate_hz = 0", "");
    const std::string errors = ::testing::internal::GetCapturedStderr();

    EXPECT_EQ(exitCode, hypha::kExitConfiguration);
    EXPECT_NE(errors.find("cycle.rate_hz"), std::string::npos) << errors;
}

TEST_F(CycleNodeConfigTables, AConfiguredPortNameSelectsItsOwnSupervisionTable)
{
    this->path = std::filesystem::temp_directory_path() / "hypha_late_bound_supervision.toml";
    std::ofstream output(this->path);
    output << "signal_port = 'actual'\n"
              "[ports.inputs.actual]\n"
              "topic='/late_bound_signal'\n"
              "silence_ms=5000\n";
    output.close();
    char        program[] = "prog";
    std::string file = this->path.string();
    char*       arguments[] = {program, file.data(), nullptr};

    EXPECT_EQ(hypha::runNode<LateBoundPortNode>(2, arguments, "late_binding_node"), 0)
        << observedSnapshot;
    EXPECT_NE(observedSnapshot.find("ports.inputs.actual.silence_ms"), std::string::npos)
        << observedSnapshot;
}

struct NoStep
{
    void operator()(const hypha::CycleTime&) const
    {
    }
};

std::vector<std::string>
bindingRow(const std::string& text, const std::string& cycle, const std::string& index)
{
    std::istringstream lines(text);
    std::string        line;
    while (std::getline(lines, line)) {
        std::istringstream       words(line);
        std::vector<std::string> cells;
        std::string              cell;
        while (words >> cell) {
            cells.push_back(cell);
        }
        const bool isRow = cells.size() == 4 && cells[0] == cycle && cells[1] == index;
        if (isRow) {
            return cells;
        }
    }
    return {};
}

class ComposedCycleOptions : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("composed_cycle_options");
    }

    void TearDown() override
    {
        this->node.reset();
    }

    std::optional<hypha::Node> node;
};

TEST_F(ComposedCycleOptions, AnOptionalTopicPortWithoutATableIsDisabled)
{
    hypha::Cycle             cycle("topic_rules_cycle");
    hypha::Arrivals<Float64> input{cycle, "cabin", hypha::optionalTopic};
    hypha::Output<Float64>   output{cycle, "cabin", hypha::optionalTopic};
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node, NoStep{}).has_value());

    EXPECT_FALSE(input.enabled());
    EXPECT_FALSE(output.enabled());
    EXPECT_FALSE(output.hasSubscribers());
}

/// What a step saw of an Arrivals port: its batch size and its binding health.
struct HealthSeen
{
    size_t                  samples{0};
    HyphaCycleBindingHealth health{};
};

struct RecordHealth
{
    hypha::Arrivals<Float64>* arrivals;
    std::vector<HealthSeen>*  seen;

    void operator()(const hypha::CycleTime&) const
    {
        const auto health = this->arrivals->bindingHealth();
        if (!health) {
            throw std::runtime_error("bindingHealth() failed in step()");
        }
        this->seen->push_back(HealthSeen{this->arrivals->samples().size(), *health});
    }
};

TEST_F(ComposedCycleOptions, BindingHealthInAStepCountsThisStepsWorkLimitDrops)
{
    const std::string topic = hypha_test::testTopic("/direct_input_health");
    auto              publisher = hypha::Node().advertise<Float64>(hypha::Topic{topic});
    hypha::Cycle      cycle("direct_input_health_cycle");
    // The subscriber of a topic queues the deepest perStep() of its ports. The second
    // port shares the topic and takes one sample of the three queued for the first.
    hypha::Arrivals<Float64> wide{
        cycle, hypha::Topic{topic}, hypha::perStep(3), hypha::unsupervised
    };
    hypha::Arrivals<Float64> arrivals{
        cycle, "health", hypha::sharing(wide), hypha::perStep(1), hypha::unsupervised
    };
    EXPECT_EQ(arrivals.bindingHealth().error(), hypha::Status::InvalidState);
    std::vector<HealthSeen> seen;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node, RecordHealth{&arrivals, &seen}).has_value());

    for (const double value : {1.0, 2.0, 3.0}) {
        ASSERT_TRUE(publisher.publish(Float64{value}));
    }
    for (int round = 0; round < 10 && (seen.empty() || seen.back().samples == 0); ++round) {
        hypha_test::advance(hypha::Duration::fromMillis(10));
        ASSERT_TRUE(this->node->runFor(hypha::Duration::fromMillis(20)).ok());
    }

    ASSERT_FALSE(seen.empty());
    const HealthSeen& first = seen.back();
    ASSERT_EQ(first.samples, 1U);
    // Over the bound the oldest samples go: the step sees the newest.
    EXPECT_EQ(arrivals.samples().front().data, 3.0);
    EXPECT_EQ(first.health.capture.work_limit_drops, 2U);
    EXPECT_EQ(first.health.work_limit_drops_total, 2U);
    EXPECT_EQ(first.health.age_rejections_total, 0U);

    const auto between = arrivals.bindingHealth();
    ASSERT_TRUE(between.has_value());
    EXPECT_EQ(between->work_limit_drops_total, 2U);
}

TEST_F(ComposedCycleOptions, ADisabledPortReportsZeroBindingHealth)
{
    hypha::Cycle             cycle("disabled_health_cycle");
    hypha::Arrivals<Float64> cabin{cycle, "cabin", hypha::optionalTopic};
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node, NoStep{}).has_value());

    ASSERT_FALSE(cabin.enabled());
    const auto health = cabin.bindingHealth();
    ASSERT_TRUE(health.has_value());
    EXPECT_EQ(health->work_limit_drops_total, 0U);
}

struct CountSubscribers
{
    hypha::Output<Float64>* output;
    std::optional<size_t>*  count;

    void operator()(const hypha::CycleTime&) const
    {
        *this->count = this->output->subscriberCount();
    }
};

TEST_F(ComposedCycleOptions, AnOutputReportsItsConnectedSubscribers)
{
    const std::string topic = hypha_test::testTopic("/counted_output");
    const auto        file = std::filesystem::temp_directory_path() / "hypha_counted_output.toml";
    std::ofstream(file) << "[ports.outputs.counted]\ntopic='" << topic << "'\n";
    this->node.reset();
    this->node.emplace("counted_output_node", file.string());
    hypha::Cycle           cycle("counted_output_cycle");
    hypha::Output<Float64> output{cycle, "counted"};
    auto                   listener = hypha::Node().subscribe<Float64>(hypha::Topic{topic});
    std::optional<size_t>  count;
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node, CountSubscribers{&output, &count}).has_value());

    ASSERT_TRUE(this->node->runFor(hypha::Duration::fromMillis(50)).ok());
    hypha_test::advance(hypha::Duration::fromMillis(20));
    ASSERT_TRUE(this->node->runFor(hypha::Duration::fromMillis(50)).ok());

    std::filesystem::remove(file);
    ASSERT_TRUE(count.has_value());
    EXPECT_TRUE(output.hasSubscribers());
    EXPECT_EQ(*count, 1U);
}

class SharedSourceRoles : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("shared_source_roles");
    }

    void TearDown() override
    {
        this->node.reset();
    }

    std::optional<hypha::Node> node;
};

TEST_F(SharedSourceRoles, EachBindingShowsItsOwnRoleOnOneSource)
{
    const std::string      topic = hypha_test::testTopic("/shared");
    hypha::Cycle           cycle("roles_cycle");
    hypha::Latest<Float64> needed{cycle, "needed", hypha::Topic{topic}, hypha::unsupervised};
    hypha::Latest<Float64> extra{cycle, "extra", hypha::sharing(needed), hypha::advisory};
    cycle.setRate(hypha::Hz(100.0));
    ASSERT_TRUE(cycle.start(*this->node, NoStep{}).has_value());

    const std::string text = test_support::introspectionText();

    const std::vector<std::string> first = bindingRow(text, "roles_cycle", "0");
    const std::vector<std::string> second = bindingRow(text, "roles_cycle", "1");
    ASSERT_EQ(first.size(), 4U) << text;
    ASSERT_EQ(second.size(), 4U) << text;
    EXPECT_EQ(first[2], topic);
    EXPECT_EQ(second[2], topic);
    EXPECT_EQ(first[3], "required");
    EXPECT_EQ(second[3], "optional");
}

}  // namespace
