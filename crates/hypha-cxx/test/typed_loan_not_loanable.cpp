// Must not compile: std_msgs/String has no field to loan. The test reads the error text.
#include <hypha/hypha.h>

#include <std_msgs/String.h>

void driver(hypha::Publisher<std_msgs::String>& pub)
{
    auto loan = pub.loan(std_msgs::String{}, 16);
    (void)pub.publish(std::move(*loan));
}
