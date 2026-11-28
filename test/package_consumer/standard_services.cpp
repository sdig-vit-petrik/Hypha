// The standard services the SDK ships, and what an application adds to a package of the
// SDK. The added definitions are generated in CMakeLists.txt with
// hypha_generate_messages(PACKAGE sensor_msgs ...), against the installed definitions.
#include "standard_services.h"

#include <nav_msgs/GetMap.h>
#include <nav_msgs/srv/get_map.hpp>
#include <sensor_msgs/FocusMeasure.h>
#include <sensor_msgs/SetCameraInfo.h>
#include <sensor_msgs/SetFocusRegion.h>
#include <sensor_msgs/srv/set_focus_region.hpp>

#include <string_view>
#include <type_traits>

bool standardServicesUsable()
{
    // The services of the SDK are ROS's own, so their MD5 is.
    const bool shipped =
        std::string_view(nav_msgs::GetMap::MD5) == "6cdd0a18e0aff5b0a3ca2326a89b54ff"
        && std::string_view(sensor_msgs::SetCameraInfo::TYPE_NAME) == "sensor_msgs/SetCameraInfo"
        && std::string_view(sensor_msgs::SetCameraInfo::MD5) == "bef1df590ed75ed1f393692395e15482";

    // The added ones sit in the same package, beside the messages of the SDK, and name one of
    // them (RegionOfInterest) with no DEPENDS.
    sensor_msgs::SetFocusRegion::Request request;
    request.region.width = 32;
    sensor_msgs::FocusMeasure measure;
    measure.focus = request.region;
    measure.sharpness = 0.25F;
    const bool added =
        std::string_view(sensor_msgs::SetFocusRegion::TYPE_NAME) == "sensor_msgs/SetFocusRegion"
        && measure.focus.width == 32U;

    // The ROS 2 names cover both: the shipped service and the added one.
    static_assert(std::is_same_v<nav_msgs::srv::GetMap::Request, nav_msgs::srv::GetMap_Request>);
    static_assert(std::is_same_v<
                  sensor_msgs::srv::SetFocusRegion::Request,
                  sensor_msgs::srv::SetFocusRegion_Request>);
    return shipped && added;
}
