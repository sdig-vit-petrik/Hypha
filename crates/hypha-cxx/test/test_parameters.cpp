#include "test_types.h"

#include <gtest/gtest.h>

#include <filesystem>
#include <fstream>
#include <optional>

namespace {

class ParameterFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_parameter_test.toml";
        std::ofstream output(this->path);
        output << "count=7\n"
                  "gain=2.5\n"
                  "name='lidar'\n"
                  "active=true\n"
                  "bad_gain='2,5'\n"
                  "bad_count='12 units'\n"
                  "bad_active='yes'\n"
                  "mandatory='not_a_number'\n"
                  "narrow_u8_overflow=300\n"
                  "unsigned_negative=-1\n"
                  "narrow_i16_overflow=100000\n"
                  "huge=1e39\n"
                  "yes=1\n"
                  "[ports.inputs.scan]\n"
                  "topic='/sensors/scan'\n"
                  "[ports.inputs.count]\n"
                  "topic=5\n"
                  "[ports.inputs.spaced]\n"
                  "topic='/has space'\n"
                  "[ports.outputs.own]\n"
                  "topic='~/status'\n";
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("parameter_test", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        std::filesystem::remove(this->path);
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

}  // namespace

TEST_F(ParameterFixture, FallsBackWhenUnset)
{
    EXPECT_EQ(hypha::config().optional<int>("missing", 42), 42);
    EXPECT_DOUBLE_EQ(hypha::config().optional<double>("missing", 1.5), 1.5);
    EXPECT_EQ(hypha::config().optional<std::string>("missing", "default"), "default");
    EXPECT_TRUE(hypha::config().optional<bool>("missing", true));
}

TEST_F(ParameterFixture, ReadsTypedValues)
{
    EXPECT_EQ(hypha::config().optional<int>("count", 0), 7);
    EXPECT_DOUBLE_EQ(hypha::config().optional<double>("gain", 0.0), 2.5);
    EXPECT_EQ(hypha::config().optional<std::string>("name", ""), "lidar");
    EXPECT_TRUE(hypha::config().optional<bool>("active", false));
}

TEST_F(ParameterFixture, AMalformedValueStopsTheNodeInsteadOfTakingTheDefault)
{
    EXPECT_THROW(hypha::config().optional<double>("bad_gain", 1.0), std::runtime_error);

    EXPECT_THROW(hypha::config().optional<int>("bad_count", 0), std::runtime_error);

    EXPECT_THROW(hypha::config().optional<bool>("bad_active", false), std::runtime_error);

    EXPECT_THROW(hypha::config().required<int>("mandatory"), std::runtime_error);

    EXPECT_THROW(hypha::config().optional<uint8_t>("narrow_u8_overflow", 0), std::runtime_error);

    EXPECT_THROW(hypha::config().optional<uint32_t>("unsigned_negative", 0), std::runtime_error);

    EXPECT_THROW(hypha::config().optional<int16_t>("narrow_i16_overflow", 0), std::runtime_error);
}

TEST_F(ParameterFixture, AnAbsentOptionalIsEmptyAndAMalformedOneThrows)
{
    EXPECT_FALSE(hypha::config().optional<int>("nope").has_value());
    EXPECT_EQ(hypha::config().optional<int>("yes"), std::optional<int>(1));
    EXPECT_TRUE(hypha::config().optional<std::string>("bad_gain").has_value());
    EXPECT_THROW(hypha::config().optional<double>("bad_gain"), hypha::ParameterError);
}

TEST_F(ParameterFixture, ATopicFromThePortsInputsTableOpensTheEndpoint)
{
    auto sub = hypha::Node().subscribe<hypha_test::Reading>("scan");
    EXPECT_TRUE(sub.enabled());
    EXPECT_EQ(sub.topic(), "/sensors/scan");
}

TEST_F(ParameterFixture, APublisherReadsTheOutputsTableAndNotTheInputsTable)
{
    auto fromOutputs = hypha::Node().advertise<hypha_test::Reading>("own");
    EXPECT_TRUE(fromOutputs.enabled());
    const std::string refusal = hypha_test::refusalOf([] {
        static_cast<void>(hypha::Node().advertise<hypha_test::Reading>("scan"));
    });
    EXPECT_NE(refusal.find("NotAvailable"), std::string::npos) << refusal;
}

TEST_F(ParameterFixture, ASubscriberReadsTheInputsTableAndNotTheOutputsTable)
{
    const std::string refusal = hypha_test::refusalOf([] {
        static_cast<void>(hypha::Node().subscribe<hypha_test::Reading>("own"));
    });
    EXPECT_NE(refusal.find("NotAvailable"), std::string::npos) << refusal;
}

TEST_F(ParameterFixture, AnUnsetOptionalTopicDisablesTheEndpoint)
{
    auto sub =
        hypha::Node().subscribe<hypha_test::Reading>("missing_input", {}, hypha::optionalTopic);
    auto pub = hypha::Node().advertise<hypha_test::Reading>("out", hypha::optionalTopic);
    EXPECT_FALSE(sub.enabled());
    EXPECT_FALSE(pub.enabled());
}

TEST_F(ParameterFixture, ARequiredPortWithoutATableIsRefusedAndTheRefusalNamesIt)
{
    const std::string refusal = hypha_test::refusalOf([] {
        static_cast<void>(hypha::Node().advertise<hypha_test::Reading>("unwired"));
    });
    EXPECT_NE(refusal.find("ports.outputs.unwired"), std::string::npos) << refusal;
    EXPECT_NE(refusal.find("NotAvailable"), std::string::npos) << refusal;
}

TEST_F(ParameterFixture, AnEmptyLiteralTopicIsRefused)
{
    const std::string refusal = hypha_test::refusalOf([] {
        static_cast<void>(hypha::Node().subscribe<hypha_test::Reading>(hypha::Topic{}));
    });
    EXPECT_NE(refusal.find("InvalidArgument"), std::string::npos) << refusal;
}

TEST_F(ParameterFixture, AValueKeepsItsTypeAndAReadOfAnotherTypeThrows)
{
    EXPECT_THROW(hypha::config().optional<std::string>("count", "fallback"), hypha::ParameterError)
        << "a number is not text";
    EXPECT_THROW(hypha::config().optional<int>("name", 0), hypha::ParameterError)
        << "text is not a number";
    EXPECT_THROW(hypha::config().optional<bool>("yes", false), hypha::ParameterError)
        << "1 is not true";
    EXPECT_DOUBLE_EQ(hypha::config().optional<double>("count", 0.0), 7.0)
        << "a whole number reads as a float";
}

TEST_F(ParameterFixture, IntegerOutOfRangeOfNarrowTypeThrows)
{
    EXPECT_THROW(hypha::config().optional<uint8_t>("narrow_u8_overflow", 0), hypha::ParameterError);
    EXPECT_THROW(
        hypha::config().optional<int16_t>("narrow_i16_overflow", 0), hypha::ParameterError
    );
    EXPECT_EQ(hypha::config().optional<int64_t>("narrow_u8_overflow", 0), 300);
}

TEST_F(ParameterFixture, NegativeIntegerAsUnsignedThrows)
{
    EXPECT_THROW(hypha::config().optional<uint32_t>("unsigned_negative", 0), hypha::ParameterError);
    EXPECT_EQ(hypha::config().optional<int64_t>("unsigned_negative", 0), -1);
}

TEST_F(ParameterFixture, FloatOverflowThrows)
{
    EXPECT_THROW(hypha::config().optional<float>("huge", 0.0F), hypha::ParameterError);
    EXPECT_DOUBLE_EQ(hypha::config().optional<double>("huge", 0.0), 1e39);
}
