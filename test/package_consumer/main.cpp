#include <hypha/hypha.h>
#include <hypha/hypha_ffi.h>

#include "ros2_style.h"
#include "short_namespace.h"

#include <consumer_msgs/Reading.h>

#include <memory>

int main()
{
    const hypha::Time current = hypha::now();
    const bool        validClock = current.nanos() >= 0;

    // The generated message resolved its std_msgs/Header field against the .msg definitions
    // the SDK installed, so the nested type is a real type here.
    consumer_msgs::Reading reading;
    reading.header.stamp.secs = 7;
    reading.value = 1.5;
    const bool nestedTypeUsable = reading.header.stamp.secs == 7;

    // The ROS 1 names are the native ones: <consumer_msgs/Reading.h> and its typedefs.
    const consumer_msgs::Reading::ConstPtr shared =
        std::make_shared<consumer_msgs::Reading>(reading);
    const bool ros1NamesUsable = shared->value == 1.5;

    const bool consumerOk = validClock && nestedTypeUsable && ros1NamesUsable
                         && ros2StyleTypesUsable() && shortNamespaceUsable() && ownGlobalHyUsable();
    return consumerOk ? 0 : 1;
}
