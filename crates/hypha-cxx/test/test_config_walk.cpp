// What a port of a node with a rich configuration reads: arrays inside the entries of a
// table list, a parameter given as one value or a list, a value from a short list, the
// tables under a scope, and a matrix (#234).

#include "hypha/config.h"
#include "hypha/hypha.h"

#include <gtest/gtest.h>

#include <cstdint>
#include <filesystem>
#include <fstream>
#include <optional>
#include <string>
#include <vector>

namespace {

class ConfigWalkFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_config_walk_test.toml";
        std::ofstream output(this->path);
        output << "baud_rate = 9600\n"
                  "baud_bad = 1200\n"
                  "mode = 'fast'\n"
                  "frame = 'base'\n"
                  "frames = ['base', 'tool']\n"
                  "gain = 2.5\n"
                  "gains = [1.0, 2.5]\n"
                  "bad_gains = [1.0, 'x']\n"
                  "mixer = [1, 0, 0, 1, 0.5, 0.5]\n"
                  "[[sensors]]\n"
                  "name = 'imu'\n"
                  "weights = [0.5, 1.5, 2.5]\n"
                  "bad_weights = [1.0, 'x']\n"
                  "topic = '/imu'\n"
                  "[[sensors]]\n"
                  "name = 'gps'\n"
                  "weights = [4.0]\n"
                  "topic = ['/a', '/b']\n"
                  "[[goals]]\n"
                  "name = 'pick'\n"
                  "[[goals.waypoints]]\n"
                  "x = 1.0\n"
                  "[[goals.waypoints]]\n"
                  "x = 2.0\n"
                  "[ports.inputs.cmd_vel]\n"
                  "topic = '/cmd_vel'\n"
                  "priority = 3\n"
                  "[ports.inputs.joy]\n"
                  "topic = '/joy'\n"
                  "priority = 7\n"
                  "[ports.outputs.out]\n"
                  "topic = '/out'\n";
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("config_walk_test", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

template <typename Read>
std::string refusal(Read read)
{
    try {
        read();
    } catch (const hypha::ParameterError& error) {
        return error.what();
    }
    return {};
}

}  // namespace

// (a) arrays, and lists of tables, inside the entries of a table list.

TEST_F(ConfigWalkFixture, AnEntryOfATableListReadsItsOwnArrays)
{
    const hypha::ConfigList sensors = hypha::config().list("sensors");
    EXPECT_EQ(sensors.at(0).requiredArray<double>("weights"), (std::vector<double>{0.5, 1.5, 2.5}));
    const auto single = sensors.at(1).requiredArray<double, 1>("weights");
    EXPECT_EQ(single[0], 4.0);
    EXPECT_FALSE(sensors.at(1).optionalArray<double>("absent").has_value());
    EXPECT_EQ(
        refusal([&sensors]() {
            sensors.at(0).requiredArray<double>("bad_weights");
        }),
        "hypha: parameter 'sensors[0].bad_weights[1]': expected a number, found \"x\""
    );
    EXPECT_EQ(
        refusal([&sensors]() {
            sensors.at(1).requiredArray<double, 3>("weights");
        }),
        "hypha: parameter 'sensors[1].weights': expected 3 elements, found 1"
    );
}

TEST_F(ConfigWalkFixture, AnEntryOfATableListHoldsAListOfTables)
{
    double sum = 0.0;
    for (const hypha::ConfigView goal : hypha::config().list("goals")) {
        for (const hypha::ConfigView waypoint : goal.list("waypoints")) {
            sum += waypoint.required<double>("x");
        }
    }
    EXPECT_DOUBLE_EQ(sum, 3.0);
    const std::string missing = refusal([]() {
        hypha::config().list("goals").at(0).list("absent");
    });
    EXPECT_EQ(missing, "hypha: required list not set: goals[0].absent");
}

// (b) one value or a list.

TEST_F(ConfigWalkFixture, AScalarOrAnArrayReadsAsAList)
{
    const hypha::ConfigView settings = hypha::config();
    EXPECT_EQ(
        settings.requiredScalarOrArray<std::string>("frame"), (std::vector<std::string>{"base"})
    );
    EXPECT_EQ(
        settings.requiredScalarOrArray<std::string>("frames"),
        (std::vector<std::string>{"base", "tool"})
    );
    EXPECT_EQ(settings.requiredScalarOrArray<double>("gain"), (std::vector<double>{2.5}));
    EXPECT_EQ(settings.requiredScalarOrArray<double>("gains"), (std::vector<double>{1.0, 2.5}));
    EXPECT_FALSE(settings.optionalScalarOrArray<double>("absent").has_value());
    const hypha::ConfigList sensors = settings.list("sensors");
    EXPECT_EQ(sensors.at(0).requiredScalarOrArray<std::string>("topic").size(), 1U);
    EXPECT_EQ(sensors.at(1).requiredScalarOrArray<std::string>("topic").size(), 2U);
}

TEST_F(ConfigWalkFixture, AScalarOrAnArrayRefusesWhatIsNotThatTypeOrIsAbsent)
{
    const hypha::ConfigView settings = hypha::config();
    EXPECT_EQ(
        refusal([&settings]() {
            settings.requiredScalarOrArray<double>("absent");
        }),
        "hypha: required parameter not set: absent"
    );
    EXPECT_EQ(
        refusal([&settings]() {
            settings.requiredScalarOrArray<double>("bad_gains");
        }),
        "hypha: parameter 'bad_gains[1]': expected a number, found \"x\""
    );
    EXPECT_NE(
        refusal([&settings]() {
            settings.requiredScalarOrArray<double>("mode");
        }).find("'mode'"),
        std::string::npos
    );
}

// (c) one of a short list.

TEST_F(ConfigWalkFixture, OneOfAcceptsTheListedValuesAndNamesThemWhenRefusing)
{
    const hypha::ConfigView settings = hypha::config();
    EXPECT_EQ(
        settings.required<uint32_t>("baud_rate", hypha::constraints::oneOf({4800, 9600, 19200})),
        9600U
    );
    EXPECT_EQ(
        refusal([&settings]() {
            settings.required<uint32_t>("baud_bad", hypha::constraints::oneOf({4800, 9600, 19200}));
        }),
        "hypha: parameter 'baud_bad' must be one of 4800, 9600, 19200"
    );
    EXPECT_EQ(
        settings.required<std::string>("mode", hypha::constraints::oneOf({"fast", "slow"})), "fast"
    );
    EXPECT_EQ(
        refusal([&settings]() {
            settings.required<std::string>("frame", hypha::constraints::oneOf({"fast", "slow"}));
        }),
        "hypha: parameter 'frame' must be one of 'fast', 'slow'"
    );
    EXPECT_EQ(settings.optional<int>("absent", 9600, hypha::constraints::oneOf({9600})), 9600);
    EXPECT_THROW(
        settings.optional<int>("absent", 1, hypha::constraints::oneOf({9600})),
        hypha::ParameterError
    );
    EXPECT_FALSE(hypha::constraints::oneOf({1, 2}).accepts(-1));
    EXPECT_TRUE(hypha::constraints::oneOf({1.5, 2.5}).accepts(2.5));
    EXPECT_THROW(hypha::constraints::oneOf<int>({}), std::invalid_argument);
}

// (d) the tables under a scope.

TEST_F(ConfigWalkFixture, TheTablesUnderAScopeAreListedByName)
{
    std::vector<std::string> keys;
    int64_t                  priorities = 0;
    for (const hypha::ConfigView& input : hypha::config("ports.inputs").tables()) {
        keys.push_back(input.key());
        priorities += input.required<int64_t>("priority");
    }
    EXPECT_EQ(keys, (std::vector<std::string>{"cmd_vel", "joy"}));
    EXPECT_EQ(priorities, 10);
    EXPECT_EQ(hypha::config("ports").tables().size(), 2U);
    EXPECT_TRUE(hypha::config("ports.inputs.joy").tables().empty());
    EXPECT_TRUE(hypha::config("never_set").tables().empty());
}

// (e) a matrix.

TEST_F(ConfigWalkFixture, AMatrixIsAFlatListWithItsShapeChecked)
{
    const hypha::ConfigView settings = hypha::config();
    const auto              fixed = settings.requiredMatrix<double>("mixer", 3, 2);
    EXPECT_EQ(fixed.rows, 3U);
    EXPECT_EQ(fixed.cols, 2U);
    EXPECT_DOUBLE_EQ(fixed.at(0, 0), 1.0);
    EXPECT_DOUBLE_EQ(fixed.at(1, 1), 1.0);
    EXPECT_DOUBLE_EQ(fixed.at(2, 0), 0.5);
    const auto anyRows = settings.requiredMatrix<double>("mixer", 0, 3);
    EXPECT_EQ(anyRows.rows, 2U);
    const auto anyCols = settings.requiredMatrix<double>("mixer", 6, 0);
    EXPECT_EQ(anyCols.cols, 1U);
    EXPECT_THROW(fixed.at(3, 0), std::out_of_range);
}

TEST_F(ConfigWalkFixture, AMatrixThatDoesNotFitIsRefusedWithItsShape)
{
    const hypha::ConfigView settings = hypha::config();
    EXPECT_EQ(
        refusal([&settings]() {
            settings.requiredMatrix<double>("mixer", 4, 2);
        }),
        "hypha: parameter 'mixer': expected 4 x 2 = 8 elements, found 6"
    );
    EXPECT_EQ(
        refusal([&settings]() {
            settings.requiredMatrix<double>("mixer", 0, 4);
        }),
        "hypha: parameter 'mixer': 6 elements do not make rows of 4 columns"
    );
    EXPECT_EQ(
        refusal([&settings]() {
            settings.requiredMatrix<double>("mixer", 4, 0);
        }),
        "hypha: parameter 'mixer': 6 elements do not make columns of 4 rows"
    );
    EXPECT_EQ(
        refusal([&settings]() {
            settings.requiredMatrix<double>("absent", 2, 2);
        }),
        "hypha: required parameter not set: absent"
    );
    EXPECT_THROW(settings.requiredMatrix<double>("mixer", 0, 0), std::invalid_argument);
}
