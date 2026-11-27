// The companion headers of sensor_msgs under their ROS 2 names, with the ROS 2 names of the
// messages from hypha_generate_ros2_headers() in CMakeLists.txt.
#include "sensor_companions.h"

#include <sensor_msgs/distortion_models.hpp>
#include <sensor_msgs/image_encodings.hpp>
#include <sensor_msgs/msg/point_cloud2.hpp>
#include <sensor_msgs/point_cloud2_iterator.hpp>
#include <sensor_msgs/point_field_conversion.hpp>

#include <cstdint>

bool sensorCompanionsRos2Usable()
{
    sensor_msgs::msg::PointCloud2    cloud;
    sensor_msgs::PointCloud2Modifier modifier(cloud);
    modifier.setPointCloud2FieldsByString(1, "xyz");
    modifier.resize(1);
    sensor_msgs::PointCloud2Iterator<float> x(cloud, "x");
    *x = 3.5F;
    const sensor_msgs::msg::PointCloud2&         constant = cloud;
    sensor_msgs::PointCloud2ConstIterator<float> read(constant, "x");

    namespace enc = sensor_msgs::image_encodings;
    return *read == 3.5F
        && sensor_msgs::typeAsPointFieldType<uint8_t>::value == sensor_msgs::msg::PointField::UINT8
        && enc::numChannels(enc::RGBA8) == 4
        && sensor_msgs::distortion_models::EQUIDISTANT == "equidistant";
}
