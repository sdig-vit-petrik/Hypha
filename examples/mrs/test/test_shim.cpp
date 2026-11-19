/// The ros/ros.h shim: time conversions, the step time, rates, package paths and throttling.
#include "check.h"

#include <ros/package.h>
#include <ros/ros.h>

#include <cstdint>

int main()
{
    MRS_CHECK(ros::Time(hypha_msgs::RosTime{3, 500000000}).toSec() == 3.5);
    MRS_CHECK(hypha_msgs::RosTime(ros::Time(2.25)).nsecs == 250000000);
    MRS_CHECK((ros::Time(5.0) - ros::Time(2.0)).toSec() == 3.0);
    MRS_CHECK((ros::Time(5.0) + ros::Duration(0.5)).toSec() == 5.5);
    MRS_CHECK(ros::Time(1, 0) < ros::Time(1, 1));
    MRS_CHECK(ros::Time(0, 0).isZero());

    // A header stamp on the left of a subtraction still yields a duration.
    const hypha_msgs::RosTime stamp = ros::Time(4.0);
    MRS_CHECK((ros::Time(6.0) - stamp).toSec() == 2.0);

    mrs::setCurrentStepTime(hypha::Time::fromSeconds(7.0));
    MRS_CHECK(ros::Time::now().toSec() == 7.0);
    mrs::setCurrentStepTime(std::nullopt);
    MRS_CHECK(ros::Time::now().toSec() != 7.0);

    MRS_CHECK(ros::Rate(100.0).expectedCycleTime().toSec() == 0.01);
    MRS_CHECK(ros::Duration(1.5).toNSec() == 1500000000);
    MRS_CHECK((ros::Duration(1.0) * 2.0).toSec() == 2.0);

    MRS_CHECK(!ros::package::getPath("mrs_uav_managers").empty());
    MRS_CHECK(ros::package::getPath("not_vendored").empty());

    mrs::setNodeName("uav1_control_manager");
    MRS_CHECK(ros::this_node::getName() == "/uav1_control_manager");

    const uint64_t before = mrs::throttledLogCount();
    for (int i = 0; i < 3; ++i) {
        ROS_INFO_THROTTLE(1.0, "[test]: throttled %d", i);
    }
    MRS_CHECK(mrs::throttledLogCount() == before + 1);
    ROS_INFO("[test]: plain %s %.2f", "text", 1.5);
    ROS_WARN_STREAM("[test]: stream " << 42);
    ROS_ERROR_STREAM_THROTTLE(1.0, "[test]: stream throttled " << 1);
    return mrsCheckResult();
}
