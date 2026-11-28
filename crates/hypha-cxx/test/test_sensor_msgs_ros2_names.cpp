// test-layer: unit (no node, no bus, no C function; #336)
// The ROS 2 names of the sensor_msgs companion headers: the .hpp paths and the
// sensor_msgs::msg names of the messages they work on. The message headers come from
// hypha_generate_ros2_headers() in CMakeLists.txt.

#include <gtest/gtest.h>
#include <sensor_msgs/distortion_models.hpp>
#include <sensor_msgs/image_encodings.hpp>
#include <sensor_msgs/msg/point_cloud2.hpp>
#include <sensor_msgs/point_cloud2_iterator.hpp>
#include <sensor_msgs/point_field_conversion.hpp>

#include <cstdint>
#include <type_traits>

namespace {

TEST(SensorMsgsRos2Names, TheIteratorsWorkOnTheMessageThatSensorMsgsMsgNames)
{
    static_assert(std::is_same_v<sensor_msgs::msg::PointCloud2, sensor_msgs::PointCloud2>);

    sensor_msgs::msg::PointCloud2    cloud;
    sensor_msgs::PointCloud2Modifier modifier(cloud);
    modifier.setPointCloud2Fields(
        2,
        "x",
        1,
        sensor_msgs::msg::PointField::FLOAT32,
        "ring",
        1,
        sensor_msgs::msg::PointField::UINT16
    );
    modifier.resize(2);

    sensor_msgs::PointCloud2Iterator<float>    x(cloud, "x");
    sensor_msgs::PointCloud2Iterator<uint16_t> ring(cloud, "ring");
    *x = 1.5F;
    *ring = 9;

    const sensor_msgs::msg::PointCloud2&         constant = cloud;
    sensor_msgs::PointCloud2ConstIterator<float> read(constant, "x");
    EXPECT_EQ(*read, 1.5F);
    EXPECT_EQ(
        sensor_msgs::typeAsPointFieldType<uint16_t>::value, sensor_msgs::msg::PointField::UINT16
    );
}

TEST(SensorMsgsRos2Names, TheEncodingsAndDistortionModelsAreTheSameNames)
{
    EXPECT_EQ(sensor_msgs::image_encodings::numChannels(sensor_msgs::image_encodings::BGR8), 3);
    EXPECT_TRUE(sensor_msgs::image_encodings::isBayer(sensor_msgs::image_encodings::BAYER_RGGB8));
    EXPECT_EQ(sensor_msgs::distortion_models::PLUMB_BOB, "plumb_bob");
}

}  // namespace
