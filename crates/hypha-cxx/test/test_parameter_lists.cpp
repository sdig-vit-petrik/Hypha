#include "test_types.h"

#include <gtest/gtest.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <optional>
#include <string>
#include <vector>

namespace {

class ParameterListFixture : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_parameter_list_test.toml";
        std::ofstream output(this->path);
        output << "gains=[1.5, 2.0]\n"
                  "counts=[3, 4]\n"
                  "wide_counts=[4294967296]\n"
                  "labels=['a', 'b']\n"
                  "flags=[true, false]\n"
                  "float_array=[0.5, 1.5, 2.5]\n"
                  "int_array=[1, 2, 3]\n"
                  "whole_array=[1, 2.5]\n"
                  "narrow_array=[1, 300]\n"
                  "bad_element=[1.0, 2.0, 'x']\n"
                  "bad_flag=[true, 1]\n"
                  "four=[1.0, 2.0, 3.0, 4.0]\n"
                  "empty=[]\n"
                  "scalar_value=5\n"
                  "[arrays]\n"
                  "offsets=[0.25, 0.75]\n"
                  "[[goals]]\n"
                  "time_s=2.0\n"
                  "name='pick'\n"
                  "[goals.limits]\n"
                  "max=3\n"
                  "[[goals]]\n"
                  "time_s=6.0\n"
                  "name='place'\n";
        output.close();
        ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
        this->node.emplace("parameter_list_test", this->path.string());
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

TEST_F(ParameterListFixture, ListEntriesSupportCheckedNarrowIntegers)
{
    const auto counts = hypha::config().list("counts");
    EXPECT_EQ(counts.at(0).required<uint32_t>(""), 3U);
    EXPECT_EQ(counts.at(1).required<int32_t>(""), 4);
    EXPECT_EQ(counts.at(1).required<size_t>(""), 4U);
    EXPECT_EQ(counts.at(0).required<long>(""), 3L);
    EXPECT_EQ(counts.at(0).optional<uint16_t>("", uint16_t{9}), 3U);
    const auto wide = hypha::config().list("wide_counts");
    EXPECT_EQ(wide.at(0).required<uint64_t>(""), 4294967296ULL);
    EXPECT_THROW(wide.at(0).required<uint32_t>(""), hypha::ParameterError);
    EXPECT_THROW(wide.at(0).required<int32_t>(""), hypha::ParameterError);
}

TEST_F(ParameterListFixture, ScalarElementsReadAsEachType)
{
    EXPECT_EQ(hypha::config().list("labels").at(1).required<std::string>(""), "b");
    EXPECT_EQ(hypha::config().list("counts").at(0).required<int64_t>(""), 3);
    EXPECT_DOUBLE_EQ(hypha::config().list("gains").at(1).required<double>(""), 2.0);
    EXPECT_FALSE(hypha::config().list("flags").at(1).required<bool>(""));
}

TEST_F(ParameterListFixture, AWholeNumberElementReadsAsAFloat)
{
    EXPECT_DOUBLE_EQ(hypha::config().list("counts").at(1).required<double>(""), 4.0);
}

TEST_F(ParameterListFixture, ATableElementFieldReadsPlainAndDottedNames)
{
    const hypha::ConfigList goals = hypha::config().list("goals");
    EXPECT_EQ(goals.at(1).required<std::string>("name"), "place");
    EXPECT_DOUBLE_EQ(goals.at(0).required<double>("time_s"), 2.0);
    EXPECT_EQ(goals.at(0).required<int64_t>("limits.max"), 3);
}

TEST_F(ParameterListFixture, AnOutOfRangeIndexIsRefusedAndAMissingFieldIsAbsent)
{
    EXPECT_THROW(hypha::config().list("counts").at(2), hypha::ParameterError);
    EXPECT_THROW(hypha::config().list("labels").at(5), hypha::ParameterError);
    EXPECT_FALSE(hypha::config().list("goals").at(0).optional<std::string>("missing").has_value());
}

TEST_F(ParameterListFixture, AWrongKindReadIsRefused)
{
    EXPECT_THROW(hypha::config().list("labels").at(0).required<int64_t>(""), hypha::ParameterError);
    EXPECT_THROW(hypha::config().list("gains").at(0).required<int64_t>(""), hypha::ParameterError);
    EXPECT_THROW(
        hypha::config().list("goals").at(0).required<double>("name"), hypha::ParameterError
    );
}

TEST_F(ParameterListFixture, ATableReadAsAScalarIsRefused)
{
    EXPECT_THROW(
        hypha::config().list("goals").at(0).required<std::string>(""), hypha::ParameterError
    );
    EXPECT_THROW(hypha::config().list("goals").at(0).required<int64_t>(""), hypha::ParameterError);
}

TEST_F(ParameterListFixture, AScalarElementWithAFieldIsAbsent)
{
    EXPECT_FALSE(hypha::config().list("counts").at(0).optional<int64_t>("x").has_value());
    EXPECT_FALSE(hypha::config().list("labels").at(0).optional<std::string>("x").has_value());
}

namespace {

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

}  // namespace

TEST_F(ParameterListFixture, ARequiredArrayOfFloatsIsAVectorInFileOrder)
{
    const std::vector<double> weights = hypha::config().requiredArray<double>("float_array");
    EXPECT_EQ(weights, (std::vector<double>{0.5, 1.5, 2.5}));
}

TEST_F(ParameterListFixture, ARequiredArrayOfIntegersIsAVectorOfThatIntegerType)
{
    const std::vector<int32_t> counts = hypha::config().requiredArray<int32_t>("int_array");
    EXPECT_EQ(counts, (std::vector<int32_t>{1, 2, 3}));
}

TEST_F(ParameterListFixture, AWholeNumberInAFloatArrayReadsAsAFloat)
{
    const std::vector<double> values = hypha::config().requiredArray<double>("whole_array");
    EXPECT_EQ(values, (std::vector<double>{1.0, 2.5}));
}

TEST_F(ParameterListFixture, ArraysOfStringsAndFlagsReadAsTheirType)
{
    EXPECT_EQ(
        hypha::config().requiredArray<std::string>("labels"), (std::vector<std::string>{"a", "b"})
    );
    EXPECT_EQ(hypha::config().requiredArray<bool>("flags"), (std::vector<bool>{true, false}));
}

TEST_F(ParameterListFixture, AnArrayInATableIsReadThroughTheView)
{
    const std::vector<double> offsets = hypha::config("arrays").requiredArray<double>("offsets");
    EXPECT_EQ(offsets, (std::vector<double>{0.25, 0.75}));
}

TEST_F(ParameterListFixture, AFixedSizeArrayOfTheRightLengthIsAStdArray)
{
    const std::array<double, 3> values = hypha::config().requiredArray<double, 3>("float_array");
    EXPECT_EQ(values, (std::array<double, 3>{0.5, 1.5, 2.5}));
}

TEST_F(ParameterListFixture, AFixedSizeArrayOfAnotherLengthIsRefusedWithBothLengths)
{
    const std::string text = refusal([]() {
        static_cast<void>(hypha::config().requiredArray<double, 3>("four"));
    });
    EXPECT_EQ(text, "hypha: parameter 'four': expected 3 elements, found 4");
    const std::string optionalText = refusal([]() {
        static_cast<void>(hypha::config().optionalArray<double, 5>("four"));
    });
    EXPECT_EQ(optionalText, "hypha: parameter 'four': expected 5 elements, found 4");
}

TEST_F(ParameterListFixture, AWrongElementNamesItsIndexAndShowsWhatItFound)
{
    const std::string text = refusal([]() {
        static_cast<void>(hypha::config().requiredArray<double>("bad_element"));
    });
    EXPECT_EQ(text, "hypha: parameter 'bad_element[2]': expected a number, found \"x\"");
    const std::string flag = refusal([]() {
        static_cast<void>(hypha::config().requiredArray<bool>("bad_flag"));
    });
    EXPECT_EQ(flag, "hypha: parameter 'bad_flag[1]': expected a boolean, found 1");
}

TEST_F(ParameterListFixture, AnElementOutOfTheRangeOfTheIntegerTypeIsRefused)
{
    EXPECT_EQ(
        (hypha::config().requiredArray<uint16_t>("narrow_array")), (std::vector<uint16_t>{1, 300})
    );
    const std::string text = refusal([]() {
        static_cast<void>(hypha::config().requiredArray<uint8_t>("narrow_array"));
    });
    EXPECT_EQ(
        text, "hypha: parameter 'narrow_array[1]': expected an integer in [0, 255], found 300"
    );
}

TEST_F(ParameterListFixture, AnArrayOfTablesIsRefusedAsNotAScalarList)
{
    const std::string text = refusal([]() {
        static_cast<void>(hypha::config().requiredArray<double>("goals"));
    });
    EXPECT_EQ(text, "hypha: parameter 'goals[0]': expected a number, found a table");
    EXPECT_THROW(
        static_cast<void>(hypha::config().optionalArray<std::string>("goals")),
        hypha::ParameterError
    );
}

TEST_F(ParameterListFixture, AnAbsentArrayIsRequiredOrEmptyButNeverMalformed)
{
    const std::string text = refusal([]() {
        static_cast<void>(hypha::config().requiredArray<double>("nothing"));
    });
    EXPECT_EQ(text, "hypha: required parameter not set: nothing");
    EXPECT_THROW(
        static_cast<void>(hypha::config().requiredArray<double, 3>("nothing")),
        hypha::ParameterError
    );
    EXPECT_FALSE(hypha::config().optionalArray<double>("nothing").has_value());
    EXPECT_FALSE((hypha::config().optionalArray<double, 3>("nothing").has_value()));
}

TEST_F(ParameterListFixture, AScalarWhereAnArrayIsAskedIsMalformedForRequiredAndOptional)
{
    const std::string text = refusal([]() {
        static_cast<void>(hypha::config().requiredArray<int64_t>("scalar_value"));
    });
    EXPECT_EQ(text, "hypha: parameter 'scalar_value' is not a list");
    EXPECT_THROW(
        static_cast<void>(hypha::config().optionalArray<int64_t>("scalar_value")),
        hypha::ParameterError
    );
}

TEST_F(ParameterListFixture, AnOptionalArrayHoldsTheValuesWhenTheKeyIsSet)
{
    const std::optional<std::vector<double>> weights =
        hypha::config().optionalArray<double>("float_array");
    ASSERT_TRUE(weights.has_value());
    EXPECT_EQ(weights->size(), 3U);
    const std::optional<std::array<double, 3>> fixed =
        hypha::config().optionalArray<double, 3>("float_array");
    ASSERT_TRUE(fixed.has_value());
    EXPECT_DOUBLE_EQ((*fixed)[2], 2.5);
}

TEST_F(ParameterListFixture, AnEmptyArrayIsAnEmptyVectorAndALengthErrorForAFixedSize)
{
    const std::vector<double> none = hypha::config().requiredArray<double>("empty");
    EXPECT_TRUE(none.empty());
    const std::optional<std::vector<double>> optionalNone =
        hypha::config().optionalArray<double>("empty");
    ASSERT_TRUE(optionalNone.has_value());
    EXPECT_TRUE(optionalNone->empty());
    const std::string text = refusal([]() {
        static_cast<void>(hypha::config().requiredArray<double, 3>("empty"));
    });
    EXPECT_EQ(text, "hypha: parameter 'empty': expected 3 elements, found 0");
    const std::array<double, 0> zero = hypha::config().requiredArray<double, 0>("empty");
    EXPECT_TRUE(zero.empty());
}

TEST_F(ParameterListFixture, ReadingAnArrayMarksItsKeyAsRead)
{
    const std::vector<std::string> before = hypha::config().unreadKeys();
    EXPECT_NE(std::ranges::find(before, "int_array"), before.end());
    static_cast<void>(hypha::config().requiredArray<int64_t>("int_array"));
    const std::vector<std::string> after = hypha::config().unreadKeys();
    EXPECT_EQ(std::ranges::find(after, "int_array"), after.end());
    EXPECT_NE(std::ranges::find(after, "float_array"), after.end());
    static_cast<void>(hypha::config().optionalArray<double>("float_array"));
    const std::vector<std::string> last = hypha::config().unreadKeys();
    EXPECT_EQ(std::ranges::find(last, "float_array"), last.end());
}

TEST_F(ParameterListFixture, AnArrayInsideATableEntryIsRefusedLikeANestedList)
{
    const hypha::ConfigList goals = hypha::config().list("goals");
    EXPECT_THROW(
        static_cast<void>(goals.at(0).requiredArray<double>("limits")), hypha::ParameterError
    );
}
