// The PointCloud2 iterators read and write a field where the ROS 1 encoding puts it, at any
// byte offset: the array of a received view starts after the header's strings and the
// `fields` array, and a field may sit at an odd offset of a point. They go through
// std::memcpy, so no access is misaligned (test_typed_loan_sanitized runs this file under
// UBSan, which reports a misaligned load or store).
#include <gtest/gtest.h>
#include <sensor_msgs/PointCloud2.h>
#include <sensor_msgs/point_cloud2_iterator.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <span>
#include <vector>

namespace {

using sensor_msgs::PointCloud2;
using sensor_msgs::PointCloud2ConstIterator;
using sensor_msgs::PointCloud2Iterator;
using sensor_msgs::PointField;

constexpr size_t kPoints = 5;

// x, y, z (float32) at 0, 4, 8, intensity (float32) at 12, ring (uint16) at 16, scale
// (float64) at 18: a 26 byte point, so most fields of most points are misaligned wherever
// the array starts.
constexpr uint32_t kStep = 26;

PointField field(const char* name, uint32_t offset, uint8_t datatype)
{
    PointField result;
    result.name = name;
    result.offset = offset;
    result.datatype = datatype;
    result.count = 1;
    return result;
}

std::vector<PointField> fieldTable(uint32_t shift)
{
    return {
        field("x", shift + 0, PointField::FLOAT32),
        field("y", shift + 4, PointField::FLOAT32),
        field("z", shift + 8, PointField::FLOAT32),
        field("intensity", shift + 12, PointField::FLOAT32),
        field("ring", shift + 16, PointField::UINT16),
        field("scale", shift + 18, PointField::FLOAT64),
    };
}

template <typename T>
void put(std::vector<uint8_t>& bytes, size_t at, T value)
{
    std::memcpy(bytes.data() + at, &value, sizeof(T));
}

template <typename T>
T take(const uint8_t* bytes, size_t at)
{
    T value;
    std::memcpy(&value, bytes + at, sizeof(T));
    return value;
}

float xOf(size_t point)
{
    return 1.5F + static_cast<float>(point);
}

float yOf(size_t point)
{
    return -2.25F * static_cast<float>(point);
}

float zOf(size_t point)
{
    return 100.0F + static_cast<float>(point) / 8.0F;
}

float intensityOf(size_t point)
{
    return 0.5F + static_cast<float>(point);
}

uint16_t ringOf(size_t point)
{
    return static_cast<uint16_t>(0x1234U + point);
}

double scaleOf(size_t point)
{
    return 1e-3 * static_cast<double>(point + 1);
}

/// A byte buffer whose points start @p start bytes into it, a received cloud's array.
std::vector<uint8_t> filledBytes(size_t start)
{
    std::vector<uint8_t> bytes(start + kPoints * kStep);
    for (size_t point = 0; point < kPoints; ++point) {
        const size_t at = start + point * kStep;
        put(bytes, at + 0, xOf(point));
        put(bytes, at + 4, yOf(point));
        put(bytes, at + 8, zOf(point));
        put(bytes, at + 12, intensityOf(point));
        put(bytes, at + 16, ringOf(point));
        put(bytes, at + 18, scaleOf(point));
    }
    return bytes;
}

/// A cloud message whose points start at the first byte of its array, the field table
/// shifted @p shift bytes into each point: odd offsets inside an aligned array.
PointCloud2 shiftedCloud(uint32_t shift)
{
    PointCloud2 cloud;
    cloud.height = 1;
    cloud.width = kPoints;
    cloud.point_step = shift + kStep;
    cloud.row_step = cloud.width * cloud.point_step;
    cloud.fields = fieldTable(shift);
    cloud.data.assign(static_cast<size_t>(cloud.row_step) * cloud.height, 0);
    return cloud;
}

TEST(PointCloud2IteratorUnaligned, AViewWhoseArrayStartsAtAnOddOffsetIsReadThroughTheIterator)
{
    constexpr size_t     start = 3;
    std::vector<uint8_t> bytes = filledBytes(start);
    ASSERT_EQ(reinterpret_cast<std::uintptr_t>(bytes.data() + start) % 2U, 1U)
        << "the vector's block is aligned, so its fourth byte is odd";

    hypha::View<PointCloud2> view;
    view.height = 1;
    view.width = kPoints;
    view.point_step = kStep;
    view.row_step = kPoints * kStep;
    view.fields = fieldTable(0);
    view.data = std::span<const uint8_t>(bytes.data() + start, kPoints * kStep);

    PointCloud2ConstIterator<float>    x(view, "x");
    PointCloud2ConstIterator<float>    y(view, "y");
    PointCloud2ConstIterator<float>    z(view, "z");
    PointCloud2ConstIterator<float>    intensity(view, "intensity");
    PointCloud2ConstIterator<uint16_t> ring(view, "ring");
    PointCloud2ConstIterator<double>   scale(view, "scale");
    size_t                             count = 0;
    for (; x != x.end(); ++x, ++y, ++z, ++intensity, ++ring, ++scale, ++count) {
        ASSERT_LT(count, kPoints);
        const float    px = *x;
        const float    py = *y;
        const float    pz = *z;
        const float    pi = *intensity;
        const uint16_t pr = *ring;
        const double   ps = *scale;
        EXPECT_EQ(px, xOf(count));
        EXPECT_EQ(py, yOf(count));
        EXPECT_EQ(pz, zOf(count));
        EXPECT_EQ(pi, intensityOf(count));
        EXPECT_EQ(pr, ringOf(count));
        EXPECT_EQ(ps, scaleOf(count));
    }
    EXPECT_EQ(count, kPoints);
}

TEST(PointCloud2IteratorUnaligned, TheOtherComponentsOfAFieldAreReachedByIndexAtAnOddOffset)
{
    std::vector<uint8_t> bytes = filledBytes(1);

    hypha::View<PointCloud2> view;
    view.height = 1;
    view.width = kPoints;
    view.point_step = kStep;
    view.row_step = kPoints * kStep;
    view.fields = fieldTable(0);
    view.data = std::span<const uint8_t>(bytes.data() + 1, kPoints * kStep);

    PointCloud2ConstIterator<float> x(view, "x");
    x += 2;
    EXPECT_EQ(x[0], xOf(2));
    EXPECT_EQ(x[1], yOf(2));
    EXPECT_EQ(x[2], zOf(2));
    EXPECT_EQ(x[3], intensityOf(2));
    PointCloud2ConstIterator<float> third = PointCloud2ConstIterator<float>(view, "x") + 2;
    EXPECT_FALSE(x != third);
}

TEST(PointCloud2IteratorUnaligned, FieldsAtOddOffsetsAreWrittenAndReadBack)
{
    for (const uint32_t shift : {1U, 3U}) {
        SCOPED_TRACE(shift);
        PointCloud2 cloud = shiftedCloud(shift);
        {
            PointCloud2Iterator<float>    x(cloud, "x");
            PointCloud2Iterator<float>    y(cloud, "y");
            PointCloud2Iterator<float>    z(cloud, "z");
            PointCloud2Iterator<float>    intensity(cloud, "intensity");
            PointCloud2Iterator<uint16_t> ring(cloud, "ring");
            PointCloud2Iterator<double>   scale(cloud, "scale");
            for (size_t point = 0; point < kPoints;
                 ++point, ++x, ++y, ++z, ++intensity, ++ring, ++scale) {
                *x = xOf(point);
                *y = yOf(point);
                *z = zOf(point);
                *intensity = intensityOf(point);
                *ring = ringOf(point);
                *scale = scaleOf(point);
            }
            EXPECT_FALSE(x != x.end());
        }

        // The bytes are where the encoding puts them.
        for (size_t point = 0; point < kPoints; ++point) {
            const size_t at = point * cloud.point_step + shift;
            EXPECT_EQ(take<float>(cloud.data.data(), at + 0), xOf(point));
            EXPECT_EQ(take<float>(cloud.data.data(), at + 4), yOf(point));
            EXPECT_EQ(take<float>(cloud.data.data(), at + 8), zOf(point));
            EXPECT_EQ(take<float>(cloud.data.data(), at + 12), intensityOf(point));
            EXPECT_EQ(take<uint16_t>(cloud.data.data(), at + 16), ringOf(point));
            EXPECT_EQ(take<double>(cloud.data.data(), at + 18), scaleOf(point));
        }

        const PointCloud2&              constant = cloud;
        PointCloud2ConstIterator<float> x(constant, "x");
        PointCloud2ConstIterator<float> z(constant, "z");
        for (size_t point = 0; point < kPoints; ++point, ++x, ++z) {
            EXPECT_EQ(*x, xOf(point));
            EXPECT_EQ(*z, zOf(point));
        }
    }
}

TEST(PointCloud2IteratorUnaligned, AnElementIsUpdatedInPlaceAtAnOddOffset)
{
    PointCloud2                cloud = shiftedCloud(1);
    PointCloud2Iterator<float> x(cloud, "x");
    x[0] = 10.0F;
    x[1] = 20.0F;
    x[2] = 30.0F;
    *x += 1.5F;
    x[1] *= 2.0F;
    ++x[2];
    EXPECT_EQ(*x, 11.5F);
    EXPECT_EQ(x[1], 40.0F);
    EXPECT_EQ(x[2], 31.0F);

    PointCloud2Iterator<uint16_t> ring(cloud, "ring");
    *ring = 7;
    *ring += 3;
    EXPECT_EQ(*ring, 10U);
    PointCloud2Iterator<uint16_t> second = ring + 1;
    *second = *ring;
    EXPECT_EQ(take<uint16_t>(cloud.data.data(), cloud.point_step + 1 + 16), 10U);
}

}  // namespace
