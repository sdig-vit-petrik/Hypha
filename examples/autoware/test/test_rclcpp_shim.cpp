/// @file
/// The rclcpp shim without a bus: names, time and the QoS check.

#include <hypha_msgs/sensor_msgs/PointCloud2.h>

#include <gtest/gtest.h>
#include <rclcpp/rclcpp.hpp>

#include <stdexcept>

TEST(RclcppShim, RelativeNamesArePorts)
{
    EXPECT_EQ(rclcpp::detail::portName("output"), "output");
    EXPECT_EQ(rclcpp::detail::portName("~/crop_box_polygon"), "crop_box_polygon");
    EXPECT_EQ(
        rclcpp::detail::portName("crop_box_filter/debug/processing_time_ms"),
        "crop_box_filter.debug.processing_time_ms"
    );
    EXPECT_THROW((void)rclcpp::detail::portName("/absolute"), std::invalid_argument);
    EXPECT_THROW((void)rclcpp::detail::portName("~/"), std::invalid_argument);
}

TEST(RclcppShim, NodeNamesBecomeHyphaNames)
{
    EXPECT_EQ(rclcpp::detail::hyphaNodeName("crop_box_filter"), "crop_box_filter");
    EXPECT_EQ(
        rclcpp::detail::hyphaNodeName("VoxelGridDownsampleFilter"), "voxel_grid_downsample_filter"
    );
    EXPECT_EQ(rclcpp::detail::hyphaNodeName("ndt_scan_matcher2"), "ndt_scan_matcher2");
    EXPECT_THROW((void)rclcpp::detail::hyphaNodeName("my-node"), std::invalid_argument);
    EXPECT_THROW((void)rclcpp::detail::hyphaNodeName("_hidden"), std::invalid_argument);
    EXPECT_THROW((void)rclcpp::detail::hyphaNodeName(""), std::invalid_argument);
}

TEST(RclcppShim, TimeRoundTripsThroughAStamp)
{
    builtin_interfaces::msg::Time stamp;
    stamp.secs = 12;
    stamp.nsecs = 345;
    const rclcpp::Time time(stamp);
    EXPECT_EQ(time.nanoseconds(), 12000000345);
    const builtin_interfaces::msg::Time back = time;
    EXPECT_EQ(back.secs, 12U);
    EXPECT_EQ(back.nsecs, 345U);
    EXPECT_EQ((rclcpp::Time(13, 0) - time).nanoseconds(), 999999655);
    EXPECT_EQ(rclcpp::Duration::from_seconds(1.5).nanoseconds(), 1500000000);
}

TEST(RclcppShim, AStampWithTooManyNanosecondsIsRefused)
{
    builtin_interfaces::msg::Time stamp;
    stamp.nsecs = 1000000000;
    EXPECT_THROW(rclcpp::Time{stamp}, std::invalid_argument);
}

TEST(RclcppShim, ADepthAboveThePolicyIsRefused)
{
    EXPECT_EQ(rclcpp::detail::checkedDepth<sensor_msgs::PointCloud2>("x", rclcpp::QoS(5)), 5U);
    EXPECT_THROW(
        (void)rclcpp::detail::checkedDepth<sensor_msgs::PointCloud2>("x", rclcpp::QoS(1000)),
        std::invalid_argument
    );
    EXPECT_THROW(
        (void)rclcpp::detail::checkedDepth<sensor_msgs::PointCloud2>("x", rclcpp::QoS(0)),
        std::invalid_argument
    );
}

TEST(RclcppShim, PrintfFormatting)
{
    EXPECT_EQ(rclcpp::detail::formatPrintf("%d points in %s", 3, "lidar"), "3 points in lidar");
}
