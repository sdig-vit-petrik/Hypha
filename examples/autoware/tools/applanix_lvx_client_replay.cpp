/// @file
/// The Autoware outputs of autoware/applanix's driver (`lvx_client`), from a recording of
/// its GSOF messages. The Leo Drive Isuzu bags (#46) hold the INS solution as recorded
/// GSOF 49 and 50, not the NavSatFix, GNSS/INS orientation and twist the driver publishes
/// from them. This node rebuilds the driver's GSOF solution structs from the recorded
/// messages, the inverse of the driver's toRosMessage, and runs the driver's own
/// conversions (applanix_driver_ros/conversions.cpp, unchanged) on them, as the driver's
/// GSOF 49 callbacks do with `publish_ros_msgs` and `publish_autoware_msgs` set.
///
/// Queues are 10 deep where the driver's are 100: hypha's `default` policy holds at most 32.
///
/// Stamps follow the driver's `time_source: now` of its parameter file: the framework
/// clock, which on replay is the recording's time. The recorded GSOF headers hold GPS time
/// of the week, another time base than the lidar and the IMU.

#include "applanix_driver_ros/conversions.h"

#include <applanix_msgs/msg/navigation_performance_gsof50.hpp>
#include <applanix_msgs/msg/navigation_solution_gsof49.hpp>
#include <autoware_sensing_msgs/msg/gnss_ins_orientation_stamped.hpp>
#include <geometry_msgs/msg/twist_with_covariance_stamped.hpp>
#include <rclcpp/rclcpp.hpp>
#include <sensor_msgs/msg/nav_sat_fix.hpp>

#include <optional>
#include <string>

namespace applanix_replay {

namespace gsof = applanix_driver::gsof;

/// The driver's InsSolution of a recorded GSOF 49: the fields toRosMessage copied out.
inline gsof::InsSolution toInsSolution(const applanix_msgs::msg::NavigationSolutionGsof49& in)
{
    gsof::InsSolution out{};
    out.header.type = gsof::GSOF_ID_49_INS_FULL_NAV;
    out.header.length = sizeof(gsof::InsSolution) - sizeof(gsof::Header);
    out.gps_time.week = in.gps_time.week;
    out.gps_time.time_msec = in.gps_time.time;
    out.status.imu_alignment = in.status.imu_alignment;
    out.status.gnss = in.status.gnss;
    out.lla.latitude = in.lla.latitude;
    out.lla.longitude = in.lla.longitude;
    out.lla.altitude = in.lla.altitude;
    out.velocity.north = in.velocity.north;
    out.velocity.east = in.velocity.east;
    out.velocity.down = in.velocity.down;
    out.total_speed = in.total_speed;
    out.attitude.roll = in.roll;
    out.attitude.pitch = in.pitch;
    out.attitude.heading = in.heading;
    out.track_angle = in.track_angle;
    out.angular_rate.roll = in.ang_rate_long;
    out.angular_rate.pitch = in.ang_rate_trans;
    out.angular_rate.heading = in.ang_rate_down;
    out.acceleration.x = in.acc_long;
    out.acceleration.y = in.acc_trans;
    out.acceleration.z = in.acc_down;
    return out;
}

/// The driver's InsSolutionRms of a recorded GSOF 50.
inline gsof::InsSolutionRms
toInsSolutionRms(const applanix_msgs::msg::NavigationPerformanceGsof50& in)
{
    gsof::InsSolutionRms out{};
    out.header.type = gsof::GSOF_ID_50_INS_RMS;
    out.header.length = sizeof(gsof::InsSolutionRms) - sizeof(gsof::Header);
    out.gps_time.week = in.gps_time.week;
    out.gps_time.time_msec = in.gps_time.time;
    out.status.imu_alignment = in.status.imu_alignment;
    out.status.gnss = in.status.gnss;
    out.position_rms.north = in.pos_rms_error.north;
    out.position_rms.east = in.pos_rms_error.east;
    out.position_rms.down = in.pos_rms_error.down;
    out.velocity_rms.north = in.vel_rms_error.north;
    out.velocity_rms.east = in.vel_rms_error.east;
    out.velocity_rms.down = in.vel_rms_error.down;
    out.attitude_rms.roll = in.attitude_rms_error_roll;
    out.attitude_rms.pitch = in.attitude_rms_error_pitch;
    out.attitude_rms.heading = in.attitude_rms_error_heading;
    return out;
}

class LvxClientReplay : public rclcpp::Node
{
  public:
    explicit LvxClientReplay(const rclcpp::NodeOptions& options)
        : Node("lvx_client", options)
        , child_frame_id_(declare_parameter<std::string>("child_frame", "POS_REF"))
        , base_frame_id_(declare_parameter<std::string>("base_frame_id", "base_link"))
        , gnss_ins_frame_id_(declare_parameter<std::string>("gnss_ins_frame_id", "gnss_ins_link"))
        , enable_ned2enu_transform_(declare_parameter<bool>("enable_ned2enu_transform", true))
        , fix_pub_(create_publisher<sensor_msgs::msg::NavSatFix>("gnss/fix", 10))
        , orientation_pub_(
              create_publisher<autoware_sensing_msgs::msg::GnssInsOrientationStamped>(
                  "autoware_orientation", 10
              )
          )
        , twist_pub_(
              create_publisher<geometry_msgs::msg::TwistWithCovarianceStamped>(
                  "twist_with_covariance", 10
              )
          )
        , gsof50_sub_(
              create_subscription<applanix_msgs::msg::NavigationPerformanceGsof50>(
                  "gsof/ins_solution_rms_50",
                  10,
                  [this](const applanix_msgs::msg::NavigationPerformanceGsof50& message) {
                      this->rms_ = toInsSolutionRms(message);
                  }
              )
          )
        , gsof49_sub_(
              create_subscription<applanix_msgs::msg::NavigationSolutionGsof49>(
                  "gsof/ins_solution_49",
                  10,
                  [this](const applanix_msgs::msg::NavigationSolutionGsof49& message) {
                      this->publish(toInsSolution(message));
                  }
              )
          )
    {
    }

  private:
    /// The driver's GSOF 49 callbacks: NavSatFix, GNSS/INS orientation and twist, each with
    /// the latest GSOF 50 when one has arrived.
    void publish(const gsof::InsSolution& solution)
    {
        const rclcpp::Time stamp = this->now();
        namespace ros = applanix_driver_ros;

        auto fix =
            this->rms_ ? ros::toNavSatFix(solution, *this->rms_) : ros::toNavSatFix(solution);
        fix.header.frame_id = this->child_frame_id_;
        fix.header.stamp = stamp;
        this->fix_pub_->publish(fix);

        auto orientation =
            this->rms_ ? ros::toAutowareOrientationMsg(
                             solution, *this->rms_, this->enable_ned2enu_transform_
                         )
                       : ros::toAutowareOrientationMsg(solution, this->enable_ned2enu_transform_);
        orientation.header.frame_id = this->gnss_ins_frame_id_;
        orientation.header.stamp = stamp;
        this->orientation_pub_->publish(orientation);

        auto twist = ros::toTwistMsg(solution);
        twist.header.frame_id = this->base_frame_id_;
        twist.header.stamp = stamp;
        this->twist_pub_->publish(twist);
    }

    std::string                                               child_frame_id_;
    std::string                                               base_frame_id_;
    std::string                                               gnss_ins_frame_id_;
    bool                                                      enable_ned2enu_transform_;
    std::optional<gsof::InsSolutionRms>                       rms_;
    rclcpp::Publisher<sensor_msgs::msg::NavSatFix>::SharedPtr fix_pub_;
    rclcpp::Publisher<autoware_sensing_msgs::msg::GnssInsOrientationStamped>::SharedPtr
                                                                                 orientation_pub_;
    rclcpp::Publisher<geometry_msgs::msg::TwistWithCovarianceStamped>::SharedPtr twist_pub_;
    rclcpp::Subscription<applanix_msgs::msg::NavigationPerformanceGsof50>::SharedPtr gsof50_sub_;
    rclcpp::Subscription<applanix_msgs::msg::NavigationSolutionGsof49>::SharedPtr    gsof49_sub_;
};

}  // namespace applanix_replay

#include <rclcpp_components/register_node_macro.hpp>
RCLCPP_COMPONENTS_REGISTER_NODE(applanix_replay::LvxClientReplay)
