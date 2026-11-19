// ROS 1 encoder tests: the wire bytes, the sized encode path, and the checked writer.
// Each message is checked against the golden arrays in ros1_codec_golden.h on the checked
// path (encode), the sized path (detail::encodeUnchecked) and encodeInto.

#include <hypha/backend/ros1_codec.h>
#include <hypha_msgs/std_msgs/Header.h>

#include "ros1_codec_fixtures.h"
#include "ros1_codec_golden.h"

#include <gtest/gtest.h>
#include <ros/serialization.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <vector>

namespace {

using hypha::codec::decode;
using hypha::codec::encode;
using hypha::codec::encodedLength;
using hypha::codec::encodeInto;
using hypha::codec::detail::encodeUnchecked;
using ros::serialization::OStream;
using ros::serialization::Serializer;
using ros::serialization::StreamOverrunException;

/// Encodes through each entry point and compares every result with the golden bytes.
template <typename M, std::size_t N>
void expectGolden(const M& message, const uint8_t (&golden)[N], std::size_t size)
{
    ASSERT_LE(size, N);
    const std::vector<uint8_t> want(golden, golden + size);

    const std::vector<uint8_t> checked = encode(message);
    EXPECT_EQ(checked, want);
    EXPECT_EQ(encodedLength(message), size);

    std::vector<uint8_t> sized(size, 0xEEU);
    EXPECT_TRUE(encodeUnchecked(message, sized.data(), size));
    EXPECT_EQ(sized, want);

    std::vector<uint8_t> exact(size, 0xEEU);
    EXPECT_TRUE(encodeInto(message, exact.data(), size));
    EXPECT_EQ(exact, want);
}

/// Decodes the golden bytes into M, then checks that M encodes back to them.
template <typename M, std::size_t N>
void expectRoundTrip(const uint8_t (&golden)[N], std::size_t size)
{
    ASSERT_LE(size, N);
    const std::vector<uint8_t> want(golden, golden + size);
    M                          message{};
    ASSERT_TRUE(decode(want, message));
    EXPECT_EQ(encode(message), want);
}

/// The scalar serializer, one element at a time. Each element is written as its own
/// value, so this is the reference the byte-copy array path must match.
template <typename T, std::size_t N>
std::vector<uint8_t> elementwiseReference(const std::array<T, N>& value)
{
    std::vector<uint8_t> out;
    for (const T& element : value) {
        std::array<uint8_t, sizeof(T)> bytes{};
        OStream                        stream(bytes.data(), static_cast<uint32_t>(sizeof(T)));
        ros::serialization::serialize(stream, element);
        out.insert(out.end(), bytes.begin(), bytes.end());
    }
    return out;
}

template <typename T, std::size_t N>
void expectArrayMatchesElementwise(const std::array<T, N>& value)
{
    const std::vector<uint8_t> reference = elementwiseReference(value);
    const uint32_t             length = ros::serialization::serializationLength(value);
    ASSERT_EQ(length, reference.size());

    std::vector<uint8_t> actual(length, 0xEEU);
    OStream              stream(actual.data(), length);
    Serializer<std::array<T, N>>::write(stream, value);
    EXPECT_EQ(stream.getLength(), 0U);
    EXPECT_EQ(actual, reference);
}

}  // namespace

TEST(RosCodecEncode, HeaderMatchesGoldenBytes)
{
    expectGolden(ros1_fixtures::header(), ros1_golden::kHeader, ros1_golden::kHeaderSize);
    expectRoundTrip<std_msgs::Header>(ros1_golden::kHeader, ros1_golden::kHeaderSize);
}

TEST(RosCodecEncode, NestedVariableArraysMatchGoldenBytes)
{
    expectGolden(
        ros1_fixtures::float64MultiArray(),
        ros1_golden::kFloat64MultiArray,
        ros1_golden::kFloat64MultiArraySize
    );
    expectRoundTrip<std_msgs::Float64MultiArray>(
        ros1_golden::kFloat64MultiArray, ros1_golden::kFloat64MultiArraySize
    );
}

TEST(RosCodecEncode, FixedPodArraysInNestedMessagesMatchGoldenBytes)
{
    expectGolden(ros1_fixtures::odometry(), ros1_golden::kOdometry, ros1_golden::kOdometrySize);
    expectRoundTrip<nav_msgs::Odometry>(ros1_golden::kOdometry, ros1_golden::kOdometrySize);

    expectGolden(ros1_fixtures::imu(), ros1_golden::kImu, ros1_golden::kImuSize);
    expectRoundTrip<sensor_msgs::Imu>(ros1_golden::kImu, ros1_golden::kImuSize);
}

TEST(RosCodecEncode, VectorsOfStringsMatchGoldenBytes)
{
    expectGolden(
        ros1_fixtures::jointState(), ros1_golden::kJointState, ros1_golden::kJointStateSize
    );
    expectRoundTrip<sensor_msgs::JointState>(
        ros1_golden::kJointState, ros1_golden::kJointStateSize
    );
}

TEST(RosCodecEncode, BoolDurationAndNestedStampedMessagesMatchGoldenBytes)
{
    expectGolden(ros1_fixtures::boolTrue(), ros1_golden::kBoolTrue, ros1_golden::kBoolTrueSize);
    expectRoundTrip<std_msgs::Bool>(ros1_golden::kBoolTrue, ros1_golden::kBoolTrueSize);

    expectGolden(ros1_fixtures::duration(), ros1_golden::kDuration, ros1_golden::kDurationSize);
    expectRoundTrip<std_msgs::Duration>(ros1_golden::kDuration, ros1_golden::kDurationSize);

    expectGolden(
        ros1_fixtures::twistStamped(), ros1_golden::kTwistStamped, ros1_golden::kTwistStampedSize
    );
    expectRoundTrip<geometry_msgs::TwistStamped>(
        ros1_golden::kTwistStamped, ros1_golden::kTwistStampedSize
    );
}

TEST(RosCodecEncode, MessageWithNoFieldsIsZeroBytes)
{
    expectGolden(ros1_fixtures::empty(), ros1_golden::kEmpty, ros1_golden::kEmptySize);
}

TEST(RosCodecEncode, EncodeIntoRefusesACapacityThatIsNotTheEncodedLength)
{
    const auto           message = ros1_fixtures::float64MultiArray();
    const size_t         size = encodedLength(message);
    std::vector<uint8_t> buffer(size + 1U, 0xEEU);

    EXPECT_FALSE(encodeInto(message, buffer.data(), size - 1U));
    EXPECT_FALSE(encodeInto(message, buffer.data(), size + 1U));
    EXPECT_FALSE(encodeInto(message, nullptr, size));
    EXPECT_EQ(buffer.back(), 0xEEU) << "a refused encode must not write";

    ASSERT_TRUE(encodeInto(message, buffer.data(), size));
    buffer.resize(size);
    EXPECT_EQ(buffer, encode(message));
}

TEST(RosCodecEncode, UncheckedEncodeRefusesALengthTheMessageDoesNotFill)
{
    const auto   message = ros1_fixtures::odometry();
    const size_t size = ros1_golden::kOdometrySize;

    // A length past the encoding: the writes stop short of it, and that is detected.
    std::vector<uint8_t> buffer(size + 4U, 0xEEU);
    EXPECT_FALSE(encodeUnchecked(message, buffer.data(), size + 4U));

    // A null buffer, and a zero length for a message that has bytes, are refused.
    EXPECT_FALSE(encodeUnchecked(message, nullptr, size));
    EXPECT_FALSE(encodeUnchecked(ros1_fixtures::header(), nullptr, 0U));
    EXPECT_TRUE(encodeUnchecked(ros1_fixtures::empty(), nullptr, 0U));
}

TEST(RosCodecEncode, CheckedWriterStillThrowsOnAnOverrun)
{
    const auto           message = ros1_fixtures::odometry();
    const size_t         size = ros1_golden::kOdometrySize;
    std::vector<uint8_t> buffer(size);

    OStream stream(buffer.data(), static_cast<uint32_t>(size - 1U));
    EXPECT_THROW(ros::serialization::serialize(stream, message), StreamOverrunException);
}

TEST(RosCodecEncode, CheckedFixedArrayWriteStillThrowsOnAnOverrun)
{
    // The byte-copy path is a single write, so it must still be bounds checked.
    const std::array<double, 36> covariance{};
    std::vector<uint8_t>         buffer(36U * sizeof(double) - 1U);
    OStream                      stream(buffer.data(), static_cast<uint32_t>(buffer.size()));
    EXPECT_THROW(
        (Serializer<std::array<double, 36>>::write(stream, covariance)), StreamOverrunException
    );
}

TEST(RosCodecEncode, FixedNumericArraysWriteTheSameBytesAsElementwise)
{
    std::array<double, 36> covariance{};
    for (size_t i = 0; i < covariance.size(); ++i) {
        covariance[i] = static_cast<double>(i) * -1.5 + 0.25;
    }
    expectArrayMatchesElementwise(covariance);
    expectArrayMatchesElementwise(std::array<float, 4>{1.0F, -2.5F, 0.0F, 3.75F});
    expectArrayMatchesElementwise(std::array<int16_t, 5>{-1, 2, -300, 4000, 0});
    expectArrayMatchesElementwise(std::array<uint8_t, 3>{0xFFU, 0x00U, 0x7FU});
    expectArrayMatchesElementwise(std::array<int64_t, 2>{INT64_MIN, INT64_MAX});
}

TEST(RosCodecEncode, FixedBoolArrayWritesNormalisedBytes)
{
    const std::array<bool, 3> value{true, false, true};
    std::array<uint8_t, 3>    bytes{0xFFU, 0xFFU, 0xFFU};
    OStream                   stream(bytes.data(), 3U);
    Serializer<std::array<bool, 3>>::write(stream, value);
    EXPECT_EQ(bytes, (std::array<uint8_t, 3>{1U, 0U, 1U}));
    EXPECT_EQ(ros::serialization::serializationLength(value), 3U);
}

TEST(RosCodecEncode, ZeroLengthFixedArrayWritesNothing)
{
    const std::array<double, 0> none{};
    EXPECT_EQ(ros::serialization::serializationLength(none), 0U);
    OStream stream(nullptr, 0U);
    EXPECT_NO_THROW((Serializer<std::array<double, 0>>::write(stream, none)));
    EXPECT_EQ(stream.getLength(), 0U);
}
