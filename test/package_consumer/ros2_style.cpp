// Code written for ROS 2: only ROS 2 message include paths, names and pointer typedefs.
// The aliases come from hypha_generate_ros2_headers() in CMakeLists.txt.
#include "ros2_style.h"

#include <consumer_msgs/msg/reading.hpp>
#include <consumer_msgs/srv/scale.hpp>
#include <geometry_msgs/msg/vector3_stamped.hpp>
#include <std_msgs/msg/header.hpp>
#include <std_msgs/msg/string.hpp>

#include <memory>
#include <type_traits>
#include <utility>

namespace {

void onReading(const consumer_msgs::msg::Reading::ConstSharedPtr& reading, double& seen)
{
    seen = reading->value;
}

}  // namespace

bool ros2StyleTypesUsable()
{
    // A ROS 2 name is the generated type itself, not a copy of it.
    static_assert(std::is_same_v<std_msgs::msg::Header, std_msgs::Header>);
    static_assert(std::is_same_v<
                  consumer_msgs::msg::Reading_<std::allocator<void>>,
                  consumer_msgs::msg::Reading>);
    static_assert(
        std::is_same_v<consumer_msgs::srv::Scale::Request, consumer_msgs::srv::Scale_Request>
    );

    auto reading = std::make_shared<consumer_msgs::msg::Reading>();
    reading->header.frame_id = "base_link";
    reading->value = 2.0;
    double seen = 0.0;
    onReading(reading, seen);

    consumer_msgs::msg::Reading::UniquePtr owned = std::make_unique<consumer_msgs::msg::Reading>();
    owned->value = seen;

    // A standard package's type, by its rosidl header name.
    geometry_msgs::msg::Vector3Stamped::SharedPtr vector =
        std::make_shared<geometry_msgs::msg::Vector3Stamped>();
    vector->vector.x = owned->value;

    consumer_msgs::srv::Scale::Request request;
    request.factor = vector->vector.x;
    consumer_msgs::srv::Scale::Response response;
    response.scaled.value = request.factor * 2.0;

    std_msgs::msg::String text;
    text.data = "ros2";

    return seen == 2.0 && response.scaled.value == 4.0 && text.data == "ros2"
        && reading->header.frame_id == "base_link";
}
