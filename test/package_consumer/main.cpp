#include <hypha/hypha.h>
#include <hypha/hypha_ffi.h>
#include <hypha_msgs/consumer_msgs/Reading.h>

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

    const bool consumerOk = validClock && nestedTypeUsable;
    return consumerOk ? 0 : 1;
}
