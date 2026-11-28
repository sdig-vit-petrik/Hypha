// test-layer: unit (no node, no bus, no C function; #336)
// The companion headers of sensor_msgs that ship beside the messages, ros/common_msgs
// unchanged: the PointCloud2 iterators and modifier, the PointField conversions, the
// image encodings and the distortion models, under their ROS 1 names.

#include <gtest/gtest.h>
#include <sensor_msgs/PointCloud2.h>
#include <sensor_msgs/PointField.h>
#include <sensor_msgs/distortion_models.h>
#include <sensor_msgs/image_encodings.h>
#include <sensor_msgs/point_cloud2_iterator.h>
#include <sensor_msgs/point_field_conversion.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <stdexcept>
#include <string>
#include <type_traits>

namespace {

namespace enc = sensor_msgs::image_encodings;
using sensor_msgs::PointCloud2;
using sensor_msgs::PointCloud2ConstIterator;
using sensor_msgs::PointCloud2Iterator;
using sensor_msgs::PointCloud2Modifier;
using sensor_msgs::PointField;

PointCloud2 xyzCloud(size_t points)
{
    PointCloud2 cloud;
    cloud.header.frame_id = "lidar";
    cloud.height = 1;
    PointCloud2Modifier modifier(cloud);
    modifier.setPointCloud2FieldsByString(1, "xyz");
    modifier.resize(points);
    return cloud;
}

TEST(PointCloud2Modifier, SetsTheFieldTableOfXyzAndSizesTheBuffer)
{
    const PointCloud2 cloud = xyzCloud(4);

    ASSERT_EQ(cloud.fields.size(), 3U);
    EXPECT_EQ(cloud.fields[0].name, "x");
    EXPECT_EQ(cloud.fields[1].name, "y");
    EXPECT_EQ(cloud.fields[2].name, "z");
    EXPECT_EQ(cloud.fields[0].offset, 0U);
    EXPECT_EQ(cloud.fields[1].offset, 4U);
    EXPECT_EQ(cloud.fields[2].offset, 8U);
    for (const PointField& field : cloud.fields) {
        EXPECT_EQ(field.datatype, PointField::FLOAT32);
        EXPECT_EQ(field.count, 1U);
    }
    // The string form pads "xyz" to 16 bytes, as ROS 1 does since Hydro.
    EXPECT_EQ(cloud.point_step, 16U);
    EXPECT_EQ(cloud.width, 4U);
    EXPECT_EQ(cloud.row_step, 64U);
    EXPECT_EQ(cloud.data.size(), 64U);
}

TEST(PointCloud2Modifier, PadsAFieldTableGivenByNameTypeAndCount)
{
    PointCloud2         cloud;
    PointCloud2Modifier modifier(cloud);
    modifier.setPointCloud2Fields(
        3,
        "x",
        1,
        PointField::FLOAT32,
        "ring",
        1,
        PointField::UINT16,
        "intensity",
        1,
        PointField::FLOAT64
    );
    modifier.resize(2);

    ASSERT_EQ(cloud.fields.size(), 3U);
    EXPECT_EQ(cloud.fields[0].offset, 0U);
    EXPECT_EQ(cloud.fields[1].offset, 4U);
    EXPECT_EQ(cloud.fields[2].offset, 6U);
    EXPECT_EQ(cloud.point_step, 14U);
    EXPECT_EQ(modifier.size(), 2U);

    modifier.clear();
    EXPECT_EQ(modifier.size(), 0U);
    EXPECT_TRUE(cloud.data.empty());
}

TEST(PointCloud2Modifier, TheFieldsOfRgbAndAXyzrgbCloudFollowTheStringNames)
{
    PointCloud2         cloud;
    PointCloud2Modifier modifier(cloud);
    modifier.setPointCloud2FieldsByString(2, "xyz", "rgb");
    modifier.resize(1);

    ASSERT_EQ(cloud.fields.size(), 4U);
    EXPECT_EQ(cloud.fields[3].name, "rgb");
    EXPECT_EQ(cloud.fields[3].offset, 16U);
    EXPECT_EQ(cloud.point_step, 32U);
}

TEST(PointCloud2Iterator, WritesEachFieldAndTheConstIteratorReadsItBack)
{
    PointCloud2                cloud = xyzCloud(3);
    PointCloud2Iterator<float> x(cloud, "x");
    PointCloud2Iterator<float> y(cloud, "y");
    PointCloud2Iterator<float> z(cloud, "z");
    for (size_t i = 0; i < 3; ++i, ++x, ++y, ++z) {
        *x = static_cast<float>(i);
        *y = static_cast<float>(i) * 10.0F;
        *z = static_cast<float>(i) * 100.0F;
    }
    EXPECT_FALSE(x != x.end());

    const PointCloud2&              constant = cloud;
    PointCloud2ConstIterator<float> cx(constant, "x");
    PointCloud2ConstIterator<float> cy(constant, "y");
    PointCloud2ConstIterator<float> cz(constant, "z");
    for (size_t i = 0; i < 3; ++i, ++cx, ++cy, ++cz) {
        EXPECT_EQ(*cx, static_cast<float>(i));
        EXPECT_EQ(*cy, static_cast<float>(i) * 10.0F);
        EXPECT_EQ(*cz, static_cast<float>(i) * 100.0F);
    }
    EXPECT_FALSE(cx != cx.end());
}

TEST(PointCloud2Iterator, ReachesTheOtherComponentsOfAFieldAndAdvancesByStride)
{
    PointCloud2                cloud = xyzCloud(4);
    PointCloud2Iterator<float> iter(cloud, "x");
    iter += 2;
    iter[0] = 7.0F;
    iter[1] = 8.0F;
    iter[2] = 9.0F;

    // The bytes of point 2 hold x, y, z in order: the iterator reached all three.
    std::array<float, 3> third{};
    std::memcpy(third.data(), &cloud.data[2 * cloud.point_step], sizeof(third));
    EXPECT_EQ(third[0], 7.0F);
    EXPECT_EQ(third[1], 8.0F);
    EXPECT_EQ(third[2], 9.0F);
}

TEST(PointCloud2Iterator, ReadsAndWritesAnInteger)
{
    PointCloud2         cloud;
    PointCloud2Modifier modifier(cloud);
    modifier.setPointCloud2Fields(2, "x", 1, PointField::FLOAT32, "ring", 1, PointField::UINT16);
    modifier.resize(2);

    PointCloud2Iterator<uint16_t> ring(cloud, "ring");
    *ring = 5;
    ++ring;
    *ring = 6;

    const PointCloud2&                 constant = cloud;
    PointCloud2ConstIterator<uint16_t> check(constant, "ring");
    EXPECT_EQ(*check, 5U);
    ++check;
    EXPECT_EQ(*check, 6U);
}

TEST(PointCloud2Iterator, RefusesAFieldTheCloudDoesNotHave)
{
    PointCloud2 cloud = xyzCloud(1);
    EXPECT_THROW(PointCloud2Iterator<float>(cloud, "intensity"), std::runtime_error);
}

// Every PointField datatype, through its own value and through float and double.
template <typename T, int Datatype>
void roundTripsAField(T value)
{
    std::array<unsigned char, sizeof(T)> buffer{};
    sensor_msgs::writePointCloud2BufferValue<Datatype, T>(buffer.data(), value);
    EXPECT_EQ((sensor_msgs::readPointCloud2BufferValue<Datatype, T>(buffer.data())), value);

    std::array<unsigned char, sizeof(T)> by_tag{};
    sensor_msgs::writePointCloud2BufferValue<T>(by_tag.data(), Datatype, value);
    EXPECT_EQ(buffer, by_tag);
    EXPECT_EQ(sensor_msgs::readPointCloud2BufferValue<T>(by_tag.data(), Datatype), value);
}

TEST(PointFieldConversion, ReadsAndWritesAFieldOfEveryDatatype)
{
    roundTripsAField<int8_t, PointField::INT8>(-100);
    roundTripsAField<uint8_t, PointField::UINT8>(200);
    roundTripsAField<int16_t, PointField::INT16>(-30000);
    roundTripsAField<uint16_t, PointField::UINT16>(60000);
    roundTripsAField<int32_t, PointField::INT32>(-2'000'000'000);
    roundTripsAField<uint32_t, PointField::UINT32>(4'000'000'000U);
    roundTripsAField<float, PointField::FLOAT32>(1.5F);
    roundTripsAField<double, PointField::FLOAT64>(-2.25);
}

TEST(PointFieldConversion, ConvertsTheStoredTypeToTheRequestedOne)
{
    std::array<unsigned char, sizeof(float)> buffer{};
    sensor_msgs::writePointCloud2BufferValue<PointField::FLOAT32, float>(buffer.data(), 3.0F);
    EXPECT_EQ(
        sensor_msgs::readPointCloud2BufferValue<double>(buffer.data(), PointField::FLOAT32), 3.0
    );
    EXPECT_EQ(
        sensor_msgs::readPointCloud2BufferValue<int32_t>(buffer.data(), PointField::FLOAT32), 3
    );

    std::array<unsigned char, sizeof(uint16_t)> narrow{};
    sensor_msgs::writePointCloud2BufferValue<uint16_t>(narrow.data(), PointField::UINT16, 513.0);
    EXPECT_EQ(
        (sensor_msgs::readPointCloud2BufferValue<PointField::UINT16, uint16_t>(narrow.data())), 513U
    );
}

TEST(PointFieldConversion, MapsADatatypeToItsTypeAndBack)
{
    static_assert(
        std::is_same_v<sensor_msgs::pointFieldTypeAsType<PointField::INT8>::type, int8_t>
    );
    static_assert(
        std::is_same_v<sensor_msgs::pointFieldTypeAsType<PointField::FLOAT64>::type, double>
    );
    EXPECT_EQ(sensor_msgs::typeAsPointFieldType<uint16_t>::value, PointField::UINT16);
    EXPECT_EQ(sensor_msgs::typeAsPointFieldType<float>::value, PointField::FLOAT32);
}

TEST(ImageEncodings, GivesTheStandardNamesTheirDepthAndChannels)
{
    struct Row
    {
        std::string name;
        int         depth;
        int         channels;
    };

    const std::array<Row, 14> rows = {{
        {enc::MONO8, 8, 1},
        {enc::MONO16, 16, 1},
        {enc::BGR8, 8, 3},
        {enc::RGB8, 8, 3},
        {enc::BGRA8, 8, 4},
        {enc::RGBA8, 8, 4},
        {enc::RGB16, 16, 3},
        {enc::BGRA16, 16, 4},
        {enc::BAYER_RGGB8, 8, 1},
        {enc::BAYER_GRBG16, 16, 1},
        {enc::TYPE_8UC3, 8, 3},
        {enc::TYPE_16SC2, 16, 2},
        {enc::TYPE_32FC1, 32, 1},
        {enc::TYPE_64FC4, 64, 4},
    }};
    for (const Row& row : rows) {
        EXPECT_EQ(enc::bitDepth(row.name), row.depth) << row.name;
        EXPECT_EQ(enc::numChannels(row.name), row.channels) << row.name;
    }
    EXPECT_EQ(enc::bitDepth(enc::YUV422), 8);
    EXPECT_EQ(enc::numChannels(enc::YUV422), 2);
}

TEST(ImageEncodings, ClassifiesTheStandardNames)
{
    EXPECT_TRUE(enc::isMono(enc::MONO8));
    EXPECT_TRUE(enc::isMono(enc::MONO16));
    EXPECT_FALSE(enc::isMono(enc::BGR8));

    EXPECT_TRUE(enc::isColor(enc::BGR8));
    EXPECT_TRUE(enc::isColor(enc::RGBA16));
    EXPECT_FALSE(enc::isColor(enc::MONO8));
    EXPECT_FALSE(enc::isColor(enc::BAYER_RGGB8));

    EXPECT_TRUE(enc::isBayer(enc::BAYER_RGGB8));
    EXPECT_TRUE(enc::isBayer(enc::BAYER_GBRG16));
    EXPECT_FALSE(enc::isBayer(enc::RGB8));

    EXPECT_TRUE(enc::hasAlpha(enc::BGRA8));
    EXPECT_FALSE(enc::hasAlpha(enc::BGR8));
}

TEST(ImageEncodings, UsesTheNamesRosUses)
{
    EXPECT_EQ(enc::RGB8, "rgb8");
    EXPECT_EQ(enc::BGR8, "bgr8");
    EXPECT_EQ(enc::MONO8, "mono8");
    EXPECT_EQ(enc::BAYER_RGGB8, "bayer_rggb8");
    EXPECT_EQ(enc::TYPE_32FC1, "32FC1");
    EXPECT_EQ(enc::YUV422, "yuv422");
}

TEST(ImageEncodings, RefusesAnEncodingItDoesNotKnow)
{
    EXPECT_THROW(enc::bitDepth("not_an_encoding"), std::runtime_error);
    EXPECT_THROW(enc::numChannels("not_an_encoding"), std::runtime_error);
}

TEST(DistortionModels, UsesTheNamesRosUses)
{
    EXPECT_EQ(sensor_msgs::distortion_models::PLUMB_BOB, "plumb_bob");
    EXPECT_EQ(sensor_msgs::distortion_models::RATIONAL_POLYNOMIAL, "rational_polynomial");
    EXPECT_EQ(sensor_msgs::distortion_models::EQUIDISTANT, "equidistant");
}

}  // namespace
