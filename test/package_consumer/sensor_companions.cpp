// The companion headers of sensor_msgs, installed with the SDK beside the messages, under
// their ROS 1 names: nothing here names a hypha source checkout.
#include "sensor_companions.h"

#include <sensor_msgs/PointCloud2.h>
#include <sensor_msgs/distortion_models.h>
#include <sensor_msgs/image_encodings.h>
#include <sensor_msgs/point_cloud2_iterator.h>
#include <sensor_msgs/point_field_conversion.h>

#include <cstdint>
#include <cstring>

bool sensorCompanionsUsable()
{
    sensor_msgs::PointCloud2         cloud;
    sensor_msgs::PointCloud2Modifier modifier(cloud);
    modifier.setPointCloud2Fields(
        2, "x", 1, sensor_msgs::PointField::FLOAT32, "ring", 1, sensor_msgs::PointField::UINT16
    );
    modifier.resize(2);
    sensor_msgs::PointCloud2Iterator<float>    x(cloud, "x");
    sensor_msgs::PointCloud2Iterator<uint16_t> ring(cloud, "ring");
    *x = 2.5F;
    *ring = 7;
    const sensor_msgs::PointCloud2&                 constant = cloud;
    sensor_msgs::PointCloud2ConstIterator<float>    read_x(constant, "x");
    sensor_msgs::PointCloud2ConstIterator<uint16_t> read_ring(constant, "ring");

    unsigned char buffer[sizeof(double)];
    sensor_msgs::writePointCloud2BufferValue<double>(buffer, sensor_msgs::PointField::FLOAT64, 4.0);
    const double decoded =
        sensor_msgs::readPointCloud2BufferValue<double>(buffer, sensor_msgs::PointField::FLOAT64);

    namespace enc = sensor_msgs::image_encodings;
    return *read_x == 2.5F && *read_ring == 7 && decoded == 4.0 && enc::bitDepth(enc::MONO16) == 16
        && enc::numChannels(enc::BGR8) == 3 && enc::isColor(enc::RGB8) && enc::isMono(enc::MONO8)
        && enc::isBayer(enc::BAYER_RGGB8)
        && sensor_msgs::distortion_models::PLUMB_BOB == "plumb_bob";
}
