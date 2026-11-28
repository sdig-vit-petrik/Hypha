#include "hypha/callback.h"
#include "hypha/config.h"

#include "test_types.h"

#include <gtest/gtest.h>

#include <cmath>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <limits>
#include <optional>
#include <string>
#include <vector>

namespace {

class ConfigViewFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_config_view_test.toml";
        std::ofstream output(this->path);
        output << "rate_hz=50.0\n"
                  "negative=-1.0\n"
                  "label=''\n"
                  "map_file='maps/track.csv'\n"
                  "absolute_file='/opt/maps/track.csv'\n"
                  "count=7\n"
                  "scalar=3\n"
                  "[controller]\n"
                  "gain=2.5\n"
                  "[[sensors]]\n"
                  "topic='/imu'\n"
                  "rate_factor=1.0\n"
                  "[[sensors]]\n"
                  "topic='/gps'\n"
                  "ignored=true\n"
                  "rate_factor=0.5\n"
                  "[[sensors]]\n"
                  "topic='/wheel'\n"
                  "rate_factor=-2.0\n";
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("config_view_test", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

/// The text of the ParameterError that @p read throws, or an empty string.
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

bool contains(const std::string& text, const std::string& part)
{
    return text.find(part) != std::string::npos;
}

struct Counter
{
    int total{0};

    int add(int value)
    {
        this->total += value;
        return this->total;
    }

    int read() const
    {
        return this->total;
    }
};

}  // namespace

TEST_F(ConfigViewFixture, RequiredReadsAScalarAndAppliesItsConstraint)
{
    const hypha::ConfigView settings = hypha::config();
    EXPECT_DOUBLE_EQ(settings.required<double>("rate_hz", hypha::constraints::positive()), 50.0);
    EXPECT_EQ(settings.required<int64_t>("count"), 7);
}

TEST_F(ConfigViewFixture, NegativeAndNonPositiveCheckTheSign)
{
    const hypha::ConfigView settings = hypha::config();
    EXPECT_DOUBLE_EQ(settings.required<double>("negative", hypha::constraints::negative()), -1.0);
    EXPECT_DOUBLE_EQ(
        settings.required<double>("negative", hypha::constraints::nonPositive()), -1.0
    );
    EXPECT_TRUE(hypha::constraints::nonPositive().accepts(0.0));
    EXPECT_FALSE(hypha::constraints::negative().accepts(0.0));
    const std::string refused = refusal([&settings] {
        static_cast<void>(settings.required<double>("rate_hz", hypha::constraints::negative()));
    });
    EXPECT_TRUE(contains(refused, "rate_hz")) << refused;
    EXPECT_TRUE(contains(refused, "negative")) << refused;
}

TEST_F(ConfigViewFixture, AFileNameResolvesAgainstTheDirectoryOfTheParameterFile)
{
    const hypha::ConfigView settings = hypha::config();
    const std::string       expected = (this->path.parent_path() / "maps/track.csv").string();
    EXPECT_EQ(settings.path("map_file"), expected);
    EXPECT_EQ(settings.path("absolute_file"), "/opt/maps/track.csv");
    EXPECT_TRUE(contains(
        refusal([&settings] {
            static_cast<void>(settings.path("missing_file"));
        }),
        "missing_file"
    ));
    // An empty name is not a file.
    EXPECT_TRUE(contains(
        refusal([&settings] {
            static_cast<void>(settings.path("label"));
        }),
        "label"
    ));
}

TEST_F(ConfigViewFixture, RequiredRefusesAMissingValueAndNamesIt)
{
    const hypha::ConfigView settings = hypha::config();
    const std::string       text = refusal([&settings]() {
        settings.required<double>("absent");
    });
    EXPECT_TRUE(contains(text, "required parameter not set: absent")) << text;
}

TEST_F(ConfigViewFixture, RequiredRefusesAValueOfAnotherType)
{
    const hypha::ConfigView settings = hypha::config();
    const std::string       text = refusal([&settings]() {
        settings.required<std::string>("count");
    });
    EXPECT_TRUE(contains(text, "'count'")) << text;
}

TEST_F(ConfigViewFixture, AConstraintRefusalNamesTheParameterAndTheRule)
{
    const hypha::ConfigView settings = hypha::config();
    const std::string       positive = refusal([&settings]() {
        settings.required<double>("negative", hypha::constraints::positive());
    });
    EXPECT_TRUE(contains(positive, "'negative' must be finite and positive")) << positive;

    const std::string empty = refusal([&settings]() {
        settings.required<std::string>("label", hypha::constraints::nonempty());
    });
    EXPECT_TRUE(contains(empty, "'label' must not be empty")) << empty;
}

TEST_F(ConfigViewFixture, OptionalIsEmptyOnlyForAnAbsentValue)
{
    const hypha::ConfigView settings = hypha::config();
    EXPECT_FALSE(settings.optional<double>("absent").has_value());
    EXPECT_EQ(settings.optional<int64_t>("count"), std::optional<int64_t>(7));
    const std::string text = refusal([&settings]() {
        settings.optional<bool>("count");
    });
    EXPECT_TRUE(contains(text, "'count'")) << text;
}

TEST_F(ConfigViewFixture, OptionalWithADefaultValidatesTheDefault)
{
    const hypha::ConfigView settings = hypha::config();
    EXPECT_DOUBLE_EQ(settings.optional<double>("absent", 4.0, hypha::constraints::positive()), 4.0);
    EXPECT_DOUBLE_EQ(settings.optional<double>("rate_hz", 4.0), 50.0);
    const std::string text = refusal([&settings]() {
        settings.optional<double>("absent", -4.0, hypha::constraints::positive());
    });
    EXPECT_TRUE(contains(text, "'absent' must be finite and positive")) << text;
}

TEST_F(ConfigViewFixture, AScopedViewReadsUnderItsPrefix)
{
    const hypha::ConfigView controller = hypha::config("controller");
    EXPECT_DOUBLE_EQ(controller.required<double>("gain"), 2.5);
    EXPECT_EQ(controller.name("gain"), "controller.gain");
    const std::string text = refusal([&controller]() {
        controller.required<double>("absent");
    });
    EXPECT_TRUE(contains(text, "controller.absent")) << text;
}

TEST_F(ConfigViewFixture, AListIteratesInFileOrder)
{
    std::vector<std::string> topics;
    std::vector<bool>        ignored;
    for (const hypha::ConfigView sensor : hypha::config().list("sensors")) {
        topics.push_back(sensor.required<std::string>("topic", hypha::constraints::nonempty()));
        ignored.push_back(sensor.optional<bool>("ignored").value_or(false));
    }
    EXPECT_EQ(topics, (std::vector<std::string>{"/imu", "/gps", "/wheel"}));
    EXPECT_EQ(ignored, (std::vector<bool>{false, true, false}));
}

TEST_F(ConfigViewFixture, AListEntryRefusalNamesTheEntryAndTheField)
{
    const hypha::ConfigList sensors = hypha::config().list("sensors");
    ASSERT_EQ(sensors.size(), 3U);
    const std::string text = refusal([&sensors]() {
        sensors.at(2).required<double>("rate_factor", hypha::constraints::positive());
    });
    EXPECT_TRUE(contains(text, "'sensors[2].rate_factor' must be finite and positive")) << text;

    const std::string missing = refusal([&sensors]() {
        sensors.at(0).required<double>("absent");
    });
    EXPECT_TRUE(contains(missing, "sensors[0].absent")) << missing;
}

TEST_F(ConfigViewFixture, AListRefusesAMissingNameAScalarAndAnIndexPastTheEnd)
{
    const hypha::ConfigView settings = hypha::config();
    const std::string       missing = refusal([&settings]() {
        settings.list("absent");
    });
    EXPECT_TRUE(contains(missing, "required list not set: absent")) << missing;

    const std::string scalar = refusal([&settings]() {
        settings.list("scalar");
    });
    EXPECT_TRUE(contains(scalar, "'scalar' is not a list")) << scalar;

    const hypha::ConfigList sensors = settings.list("sensors");
    const std::string       past = refusal([&sensors]() {
        sensors.at(3);
    });
    EXPECT_TRUE(contains(past, "sensors[3]")) << past;
}

TEST_F(ConfigViewFixture, AListNotSetInAListEntryIsNamedByItsEntry)
{
    const hypha::ConfigList sensors = hypha::config().list("sensors");
    const std::string       text = refusal([&sensors]() {
        sensors.at(0).list("inner");
    });
    EXPECT_TRUE(contains(text, "required list not set: sensors[0].inner")) << text;
}

TEST(ConfigConstraints, NumberConstraintsRefuseNonFiniteAndOutOfRangeValues)
{
    const double infinity = std::numeric_limits<double>::infinity();
    const double nan = std::numeric_limits<double>::quiet_NaN();
    EXPECT_TRUE(hypha::constraints::finite().accepts(-3.0));
    EXPECT_FALSE(hypha::constraints::finite().accepts(nan));
    EXPECT_FALSE(hypha::constraints::positive().accepts(0.0));
    EXPECT_FALSE(hypha::constraints::positive().accepts(infinity));
    EXPECT_TRUE(hypha::constraints::range(0.0, 1.0).accepts(1.0));
    EXPECT_TRUE(hypha::constraints::range(0.0, 1.0).accepts(0.0));
    EXPECT_FALSE(hypha::constraints::range(0.0, 1.0).accepts(1.5));
    EXPECT_FALSE(hypha::constraints::range(1.0, 0.0).accepts(0.5));
    EXPECT_TRUE(hypha::constraints::nonNegative().accepts(0.0));
    EXPECT_FALSE(hypha::constraints::nonNegative().accepts(-0.1));
    EXPECT_FALSE(hypha::constraints::nonNegative().accepts(infinity));
    EXPECT_TRUE(hypha::constraints::nonempty().accepts(std::string(" ")));
    EXPECT_FALSE(hypha::constraints::nonempty().accepts(std::string()));
}

TEST(ConfigConstraints, AnIntegerRangeIsInclusiveAndComparesExactly)
{
    EXPECT_TRUE(hypha::constraints::range(1, 8).accepts(uint8_t{1}));
    EXPECT_TRUE(hypha::constraints::range(1, 8).accepts(uint8_t{8}));
    EXPECT_FALSE(hypha::constraints::range(1, 8).accepts(uint8_t{0}));
    EXPECT_FALSE(hypha::constraints::range(1, 8).accepts(uint8_t{9}));
    EXPECT_FALSE(hypha::constraints::range(8, 1).accepts(4));
    // A negative bound against an unsigned value: no wrap to a large number.
    EXPECT_TRUE(hypha::constraints::range(-5, 5).accepts(uint32_t{5}));
    EXPECT_FALSE(hypha::constraints::range(-5, -1).accepts(uint32_t{4294967295U}));
    EXPECT_TRUE(hypha::constraints::range(-5, 5).accepts(int64_t{-5}));
    EXPECT_FALSE(hypha::constraints::range(-5, 5).accepts(int64_t{-6}));
    // Beyond the 53 bits a double holds: neighbours stay distinct.
    const int64_t large = int64_t{1} << 53;
    EXPECT_TRUE(hypha::constraints::range(int64_t{0}, large).accepts(large));
    EXPECT_FALSE(hypha::constraints::range(int64_t{0}, large).accepts(large + 1));
    const uint64_t beyondSigned = uint64_t{1} << 63;
    EXPECT_FALSE(hypha::constraints::range(int64_t{0}, large).accepts(beyondSigned));
    EXPECT_THROW(hypha::constraints::range(uint64_t{0}, beyondSigned), std::invalid_argument);
    // Integer bounds still check a floating-point value.
    EXPECT_TRUE(hypha::constraints::range(0, 1).accepts(0.5));
    EXPECT_FALSE(hypha::constraints::range(0, 1).accepts(1.5));
    EXPECT_FALSE(hypha::constraints::range(0, 1).accepts(std::numeric_limits<double>::quiet_NaN()));
}

TEST_F(ConfigViewFixture, AnIntegerOutsideItsRangeIsRefusedWithTheKeyAndTheBounds)
{
    const hypha::ConfigView settings = hypha::config();
    EXPECT_EQ(settings.required<uint8_t>("count", hypha::constraints::range(1, 7)), 7);
    EXPECT_EQ(settings.optional<int32_t>("absent", 3, hypha::constraints::range(1, 7)), 3);

    const std::string above = refusal([&settings]() {
        settings.required<uint8_t>("count", hypha::constraints::range(1, 6));
    });
    EXPECT_TRUE(contains(above, "parameter 'count' must be in [1, 6]")) << above;

    const std::string fallback = refusal([&settings]() {
        settings.optional<int32_t>("absent", 9, hypha::constraints::range(1, 7));
    });
    EXPECT_TRUE(contains(fallback, "parameter 'absent' must be in [1, 7]")) << fallback;
}

TEST(MemberCallback, ForwardsArgumentsAndTheReturnValueToTheBorrowedObject)
{
    Counter counter;
    auto    add = hypha::bind<&Counter::add>(counter);
    EXPECT_EQ(add(2), 2);
    EXPECT_EQ(add(3), 5);
    const Counter& view = counter;
    auto           read = hypha::bind<&Counter::read>(view);
    EXPECT_EQ(read(), 5);
}

TEST(ConfigContextTest, LoadsParametersWithoutANodeAndReportsUnreadKeys)
{
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_config_context_test.toml";
    {
        std::ofstream output(path);
        output << "use_sim_time = false\n"
                  "rate_hz = 50.0\n"
                  "typo_root = 1\n"
                  "[node]\n"
                  "name = 'context_only'\n"
                  "[controller]\n"
                  "gain = 2.5\n"
                  "typo_gain = 3.0\n"
                  "[[sensors]]\n"
                  "topic = '/imu'\n"
                  "typo_sensor = 'bad'\n";
    }
    {
        hypha::ConfigContext context(path.string());
        EXPECT_FALSE(hypha::ok());
        EXPECT_DOUBLE_EQ(hypha::config().required<double>("rate_hz"), 50.0);
        EXPECT_DOUBLE_EQ(hypha::config("controller").required<double>("gain"), 2.5);
        const hypha::ConfigList sensors = hypha::config().list("sensors");
        ASSERT_EQ(sensors.size(), 1U);
        EXPECT_EQ(sensors.at(0).required<std::string>("topic"), "/imu");

        EXPECT_EQ(
            hypha::config().unreadKeys(),
            (std::vector<std::string>{
                "controller.typo_gain",
                "sensors[0].typo_sensor",
                "typo_root",
                "use_sim_time",
            })
        );
        EXPECT_EQ(
            hypha::config("controller").unreadKeys(),
            (std::vector<std::string>{"controller.typo_gain"})
        );
        EXPECT_EQ(sensors.at(0).unreadKeys(), (std::vector<std::string>{"sensors[0].typo_sensor"}));

        EXPECT_EQ(hypha::config().required<int64_t>("typo_root"), 1);
        EXPECT_DOUBLE_EQ(hypha::config("controller").required<double>("typo_gain"), 3.0);
        EXPECT_EQ(sensors.at(0).required<std::string>("typo_sensor"), "bad");
        EXPECT_FALSE(hypha::config().required<bool>("use_sim_time"));
        EXPECT_TRUE(hypha::config().unreadKeys().empty());
    }
    std::filesystem::remove(path);
}

TEST(ConfigContextTest, PathResolvesAgainstTheFileThatSetTheKey)
{
    const std::filesystem::path directory =
        std::filesystem::temp_directory_path() / "hypha_config_path_origin";
    std::filesystem::create_directories(directory / "sub");
    const std::filesystem::path root = directory / "root.toml";
    {
        std::ofstream output(root);
        output << "include = ['sub/a.toml']\n"
                  "root_file = 'r.txt'\n";
    }
    {
        std::ofstream output(directory / "sub" / "a.toml");
        output << "data_file = 'data.txt'\n"
                  "[model]\n"
                  "file = 'm.bin'\n"
                  "[[sensors]]\n"
                  "file = 's.bin'\n";
    }
    {
        hypha::ConfigContext context(root.string());
        EXPECT_EQ(hypha::config().path("root_file"), (directory / "r.txt").string());
        EXPECT_EQ(hypha::config().path("data_file"), (directory / "sub" / "data.txt").string());
        EXPECT_EQ(hypha::config("model").path("file"), (directory / "sub" / "m.bin").string());
        const hypha::ConfigList sensors = hypha::config().list("sensors");
        ASSERT_EQ(sensors.size(), 1U);
        EXPECT_EQ(sensors.at(0).path("file"), (directory / "sub" / "s.bin").string());
    }
    std::filesystem::remove_all(directory);
}

TEST(ConfigContextTest, RefusesAnInvalidFileAndAConflictingContext)
{
    const std::filesystem::path firstPath =
        std::filesystem::temp_directory_path() / "hypha_config_context_first.toml";
    const std::filesystem::path secondPath =
        std::filesystem::temp_directory_path() / "hypha_config_context_second.toml";
    {
        std::ofstream first(firstPath);
        first << "count = 1\n";
        std::ofstream second(secondPath);
        second << "count = 2\n";
    }
    EXPECT_THROW(
        hypha::ConfigContext("/nonexistent/hypha_config_context.toml"), std::runtime_error
    );
    {
        hypha::ConfigContext first(firstPath.string());
        hypha::ConfigContext same(firstPath.string());
        EXPECT_EQ(hypha::config().required<int64_t>("count"), 1);
        EXPECT_THROW(hypha::ConfigContext(secondPath.string()), std::runtime_error);
    }
    std::filesystem::remove(firstPath);
    std::filesystem::remove(secondPath);
}
