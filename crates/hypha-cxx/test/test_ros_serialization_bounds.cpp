// The ROS 1 decoder must refuse untrusted input before it acts on it. A length is checked
// against the bytes that remain before any allocation, and a bounds check never forms a
// pointer past the buffer.

#include <hypha/backend/ros1_codec.h>

#include <gtest/gtest.h>
#include <ros/serialization.h>
#include <rosgraph_msgs/Log.h>
#include <std_msgs/Bool.h>

#include <cstdint>
#include <cstring>
#include <string>
#include <type_traits>
#include <vector>

// ROS 1 defines `byte` as int8 and `char` as uint8. The generated Log message keeps the
// ROS constant names and types, so a byte-valued field and its constants are int8_t.
static_assert(
    std::is_same_v<decltype(rosgraph_msgs::Log{}.level), int8_t>, "byte is int8 in ROS 1"
);
static_assert(
    std::is_same_v<decltype(rosgraph_msgs::Log::DEBUG), const int8_t>,
    "a byte constant is int8 in ROS 1"
);

namespace {

using ros::serialization::IStream;
using ros::serialization::OStream;
using ros::serialization::Serializer;
using ros::serialization::StreamException;
using ros::serialization::StreamOverrunException;

template <typename T>
T readFrom(std::vector<uint8_t>& bytes, T& out)
{
    IStream stream(bytes.data(), static_cast<uint32_t>(bytes.size()));
    Serializer<T>::read(stream, out);
    return out;
}

/// Little-endian encoding of a four byte length prefix.
std::vector<uint8_t> lengthPrefix(uint32_t value)
{
    return {
        static_cast<uint8_t>(value & 0xFFU),
        static_cast<uint8_t>((value >> 8) & 0xFFU),
        static_cast<uint8_t>((value >> 16) & 0xFFU),
        static_cast<uint8_t>((value >> 24) & 0xFFU)
    };
}

static_assert(sizeof(bool) == 1U, "the representation check reads one byte");

TEST(RosSerializationBounds, EveryNonzeroByteDecodesToAValidTrue)
{
    for (int candidate = 0; candidate <= 255; ++candidate) {
        std::vector<uint8_t> bytes{static_cast<uint8_t>(candidate)};
        const bool           expected = candidate != 0;
        bool                 value = !expected;

        EXPECT_NO_THROW(readFrom(bytes, value)) << "byte " << candidate;
        EXPECT_EQ(value, expected) << "byte " << candidate;

        // The decoded object must hold one of the two valid representations.
        uint8_t representation = 0xFFU;
        std::memcpy(&representation, &value, sizeof(value));
        EXPECT_EQ(representation, expected ? 1U : 0U) << "byte " << candidate;
    }
}

TEST(RosSerializationBounds, BoolRoundTripsThroughASingleCanonicalByte)
{
    std::vector<uint8_t> bytes(1, 0xFFU);
    OStream              trueOut(bytes.data(), 1);
    Serializer<bool>::write(trueOut, true);
    EXPECT_EQ(bytes[0], 1U);
    bool decoded = false;
    readFrom(bytes, decoded);
    EXPECT_TRUE(decoded);

    OStream falseOut(bytes.data(), 1);
    Serializer<bool>::write(falseOut, false);
    EXPECT_EQ(bytes[0], 0U);
    readFrom(bytes, decoded);
    EXPECT_FALSE(decoded);
}

TEST(RosSerializationBounds, ANonCanonicalTrueDecodesAsAWholeMessage)
{
    std_msgs::Bool decoded;
    decoded.data = false;

    const std::vector<uint8_t> nonCanonical{0x20U};
    ASSERT_TRUE(hypha::codec::decode(nonCanonical, decoded));
    EXPECT_TRUE(decoded.data);

    const std::vector<uint8_t> truncated{};
    EXPECT_FALSE(hypha::codec::decode(truncated, decoded));
    const std::vector<uint8_t> trailing{0x20U, 0x00U};
    EXPECT_FALSE(hypha::codec::decode(trailing, decoded));
}

TEST(RosSerializationBounds, AStringLengthLargerThanTheBufferIsRefused)
{
    std::vector<uint8_t> bytes = lengthPrefix(1024U * 1024U);
    std::string          value;

    EXPECT_THROW(readFrom(bytes, value), StreamOverrunException);
    EXPECT_TRUE(value.empty()) << "the decoder allocated for an unverified length";
}

TEST(RosSerializationBounds, AStringLengthThatFitsIsAccepted)
{
    std::vector<uint8_t> bytes = lengthPrefix(3U);
    bytes.push_back('a');
    bytes.push_back('b');
    bytes.push_back('c');
    std::string value;

    readFrom(bytes, value);
    EXPECT_EQ(value, "abc");
}

TEST(RosSerializationBounds, AFixedWidthArrayCountLargerThanTheBufferIsRefused)
{
    std::vector<uint8_t> bytes = lengthPrefix(0xFFFFFFFFU);
    std::vector<double>  value;

    EXPECT_THROW(readFrom(bytes, value), StreamOverrunException);
    EXPECT_TRUE(value.empty()) << "the decoder allocated for an unverified element count";
}

TEST(RosSerializationBounds, AVariableWidthArrayCountAboveTheBudgetIsRefused)
{
    std::vector<uint8_t>     bytes = lengthPrefix(0xFFFFFFFFU);
    std::vector<std::string> value;

    EXPECT_THROW(readFrom(bytes, value), StreamException);
    EXPECT_TRUE(value.empty()) << "the decoder allocated for an unverified element count";
}

TEST(RosSerializationBounds, AStringArrayCountThatTheBytesCannotHoldIsRefusedBeforeAnyElementExists)
{
    // A string is at least its four byte length, so 2^20 strings need 4 MiB of bytes the
    // buffer does not have. The decoder must refuse before it makes any element.
    std::vector<uint8_t>     bytes = lengthPrefix(1U << 20);
    std::vector<std::string> value;

    EXPECT_THROW(readFrom(bytes, value), StreamException);
    EXPECT_TRUE(value.empty()) << "the decoder made elements for an unverified element count";
}

/// An element of no fixed width, as a generated message is. It counts the instances alive
/// at once, so a test can see how many the decoder made before it failed.
struct CountedElement
{
    static inline int live = 0;
    static inline int peak = 0;
    uint32_t          value = 0;

    CountedElement()
    {
        track(1);
    }

    CountedElement(const CountedElement& other) : value(other.value)
    {
        track(1);
    }

    CountedElement(CountedElement&& other) noexcept : value(other.value)
    {
        track(1);
    }

    CountedElement& operator=(const CountedElement&) = default;
    CountedElement& operator=(CountedElement&&) = default;

    ~CountedElement()
    {
        track(-1);
    }

    static void track(int delta)
    {
        live += delta;
        if (peak < live) {
            peak = live;
        }
    }
};

}  // namespace

namespace ros::serialization {

template <>
struct Serializer<CountedElement>
{
    template <typename Stream>
    inline static void write(Stream& stream, const CountedElement& v)
    {
        stream.write(&v.value, 4);
    }

    template <typename Stream>
    inline static void read(Stream& stream, CountedElement& v)
    {
        stream.read(&v.value, 4);
    }

    inline static uint32_t serializedLength(const CountedElement&)
    {
        return 4;
    }
};

}  // namespace ros::serialization

namespace {

TEST(RosSerializationBounds, AnElementOfNoFixedWidthIsNotMadeForAnUnverifiedCount)
{
    // The count is inside the element budget, but the buffer holds none of the elements.
    CountedElement::peak = 0;
    std::vector<uint8_t>        bytes = lengthPrefix(1U << 20);
    std::vector<CountedElement> value;

    EXPECT_THROW(readFrom(bytes, value), StreamException);
    EXPECT_LE(CountedElement::peak, 1) << "the decoder made elements the bytes cannot hold";
}

TEST(RosSerializationBounds, AnArrayOfElementsOfNoFixedWidthDecodes)
{
    std::vector<uint8_t> bytes = lengthPrefix(2U);
    for (uint8_t byte : {1, 0, 0, 0, 2, 0, 0, 0}) {
        bytes.push_back(byte);
    }
    std::vector<CountedElement> value;

    readFrom(bytes, value);
    ASSERT_EQ(value.size(), 2U);
    EXPECT_EQ(value[0].value, 1U);
    EXPECT_EQ(value[1].value, 2U);
}

TEST(RosSerializationBounds, AnArrayCountThatFitsIsAccepted)
{
    std::vector<uint8_t> bytes = lengthPrefix(2U);
    for (int index = 0; index < 16; ++index) {
        bytes.push_back(0U);
    }
    std::vector<double> value;

    readFrom(bytes, value);
    EXPECT_EQ(value.size(), 2U);
}

TEST(RosSerializationBounds, AnEmptyBufferRefusesEveryRead)
{
    std::vector<uint8_t> bytes;
    IStream              stream(bytes.data(), 0);
    uint32_t             value = 0;

    EXPECT_THROW(stream.read(&value, 4), StreamOverrunException);
}

TEST(RosSerializationBounds, ATruncatedReadLeavesTheCursorWhereItWas)
{
    std::vector<uint8_t> bytes{1U, 2U, 3U};
    IStream              stream(bytes.data(), 3);
    uint32_t             value = 0;

    EXPECT_THROW(stream.read(&value, 4), StreamOverrunException);
    EXPECT_EQ(stream.getLength(), 3U);
}

TEST(RosSerializationBounds, AReadLengthNearTheAddressSpaceLimitIsRefused)
{
    std::vector<uint8_t> bytes{1U, 2U, 3U, 4U};
    IStream              stream(bytes.data(), 4);
    uint32_t             value = 0;

    // Forming `data_ + 0xFFFFFFFF` before comparing it is undefined behaviour rather
    // than a bounds check.
    EXPECT_THROW(stream.read(&value, 0xFFFFFFFFU), StreamOverrunException);
}

TEST(RosSerializationBounds, AWriteLengthNearTheAddressSpaceLimitIsRefused)
{
    std::vector<uint8_t> bytes{1U, 2U, 3U, 4U};
    OStream              stream(bytes.data(), 4);
    const uint32_t       value = 0;

    EXPECT_THROW(stream.write(&value, 0xFFFFFFFFU), StreamOverrunException);
}

TEST(RosSerializationBounds, ABoolArrayIsOneByteAnElementBehindItsCount)
{
    // A generated bool[] field is std::vector<bool>, whose elements are proxies.
    const std::vector<bool> flags{true, false, true};
    EXPECT_EQ(ros::serialization::serializationLength(flags), 7U);
    std::vector<uint8_t> bytes(7);
    OStream              out(bytes.data(), static_cast<uint32_t>(bytes.size()));
    ros::serialization::serialize(out, flags);
    EXPECT_EQ(bytes, (std::vector<uint8_t>{3, 0, 0, 0, 1, 0, 1}));
    EXPECT_EQ(out.getLength(), 0U);

    bytes[6] = 2;  // ROS 1 reads every nonzero byte as true
    std::vector<bool> decoded;
    readFrom(bytes, decoded);
    EXPECT_EQ(decoded, flags);
}

TEST(RosSerializationBounds, ABoolArrayCountLargerThanTheBufferIsRefused)
{
    std::vector<uint8_t> bytes = lengthPrefix(5);
    bytes.push_back(1);
    std::vector<bool> decoded;
    EXPECT_THROW(readFrom(bytes, decoded), StreamException);
}

TEST(RosSerializationBounds, ALargeVectorOfDoublesRoundTripsAsOneRun)
{
    // 4 MiB of elements: the numeric path copies the whole run in one call.
    std::vector<double> original(1U << 19);
    for (size_t index = 0; index < original.size(); ++index) {
        original[index] = static_cast<double>(index) * 0.5 - 7.25;
    }
    const uint32_t length = ros::serialization::serializationLength(original);
    ASSERT_EQ(length, 4U + 8U * original.size());

    std::vector<uint8_t> bytes(length);
    OStream              out(bytes.data(), length);
    Serializer<std::vector<double>>::write(out, original);
    EXPECT_EQ(out.getLength(), 0U);
    // The count is followed by the doubles in memory order, which is the wire order here.
    EXPECT_EQ(std::memcmp(bytes.data() + 4, original.data(), 8U * original.size()), 0);

    // A reused target of another size is resized to the count and holds the decoded run.
    std::vector<double> decoded(3, -1.0);
    IStream             in(bytes.data(), length);
    Serializer<std::vector<double>>::read(in, decoded);
    EXPECT_EQ(in.getLength(), 0U);
    EXPECT_TRUE(decoded == original);
}

TEST(RosSerializationBounds, ALargeVectorOfBytesRoundTripsIntoAReusedTarget)
{
    std::vector<uint8_t> original(4U * 1024U * 1024U);
    for (size_t index = 0; index < original.size(); ++index) {
        original[index] = static_cast<uint8_t>(index * 31U + 7U);
    }
    const uint32_t length = ros::serialization::serializationLength(original);

    std::vector<uint8_t> bytes(length);
    OStream              out(bytes.data(), length);
    Serializer<std::vector<uint8_t>>::write(out, original);
    EXPECT_EQ(out.getLength(), 0U);

    std::vector<uint8_t> decoded(7U, 0xEEU);
    IStream              in(bytes.data(), length);
    Serializer<std::vector<uint8_t>>::read(in, decoded);
    EXPECT_EQ(in.getLength(), 0U);
    EXPECT_TRUE(decoded == original);
}

TEST(RosSerializationBounds, ANumericRunShortOfItsCountLeavesTheTargetUntouched)
{
    // The count claims three doubles, and the buffer holds two and a half of them.
    std::vector<uint8_t> bytes = lengthPrefix(3U);
    bytes.resize(4U + 20U, 0U);
    std::vector<double> value{1.0, 2.0};

    EXPECT_THROW(readFrom(bytes, value), StreamOverrunException);
    EXPECT_EQ(value.size(), 2U) << "the target was resized before the count was checked";
}

TEST(RosSerializationBounds, AStringArrayRoundTripsIntoAReusedTarget)
{
    const std::vector<std::string> original{"alpha", "", "a longer string value"};
    const uint32_t                 length = ros::serialization::serializationLength(original);

    std::vector<uint8_t> bytes(length);
    OStream              out(bytes.data(), length);
    Serializer<std::vector<std::string>>::write(out, original);

    std::vector<std::string> decoded{"x", "y", "z", "w"};
    IStream                  in(bytes.data(), length);
    Serializer<std::vector<std::string>>::read(in, decoded);
    EXPECT_EQ(in.getLength(), 0U);
    EXPECT_TRUE(decoded == original);
}

}  // namespace
