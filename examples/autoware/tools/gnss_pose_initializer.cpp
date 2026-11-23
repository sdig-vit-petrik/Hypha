/// @file
/// Starts the EKF from the first GNSS pose, as autoware_pose_initializer does in its GNSS
/// mode without a map: the pose goes out as the initial pose (/initialpose3d), and the
/// EKF's trigger_node service is called with true. The map height fitter of
/// pose_initializer leaves a pose as it is without a map, so the GNSS pose is the initial
/// pose unchanged. pose_initializer itself is not ported (#110): it needs the AD API and
/// component interface utilities.
///
/// As automatic_pose_initializer retries while localization is uninitialized, the node
/// starts again when no EKF odometry arrives within a second of the trigger: an EKF that
/// restarted, or one that answered and then exited, is started anew.
///
/// A pose with a value that is not finite is skipped: the poser's first fix can meet an
/// orientation that has not arrived yet, and an EKF started from it would stay invalid.

#include <geometry_msgs/msg/pose_with_covariance_stamped.hpp>
#include <nav_msgs/msg/odometry.hpp>
#include <rclcpp/rclcpp.hpp>
#include <std_srvs/srv/set_bool.hpp>

#include <cmath>
#include <string>

namespace gnss_pose_initializer {

class GnssPoseInitializer : public rclcpp::Node
{
  public:
    explicit GnssPoseInitializer(const rclcpp::NodeOptions& options)
        : Node("gnss_pose_initializer", options)
        , initial_pose_pub_(
              create_publisher<geometry_msgs::msg::PoseWithCovarianceStamped>("initialpose", 1)
          )
        , trigger_client_(create_client<std_srvs::srv::SetBool>("ekf_trigger_node"))
        , gnss_pose_sub_(
              create_subscription<geometry_msgs::msg::PoseWithCovarianceStamped>(
                  "gnss_pose_cov",
                  1,
                  [this](const geometry_msgs::msg::PoseWithCovarianceStamped& pose) {
                      this->on_gnss_pose(pose);
                  }
              )
          )
        , odometry_sub_(
              create_subscription<nav_msgs::msg::Odometry>(
                  "ekf_odom",
                  1,
                  [this](const nav_msgs::msg::Odometry& /*odometry*/) {
                      this->odometry_seen_ = true;
                  }
              )
          )
        , watchdog_(rclcpp::create_timer(this, get_clock(), std::chrono::seconds(1), [this] {
            this->check_started();
        }))
    {
    }

  private:
    void on_gnss_pose(const geometry_msgs::msg::PoseWithCovarianceStamped& pose)
    {
        if (this->initialized_) {
            return;
        }
        if (!is_finite(pose)) {
            RCLCPP_WARN_THROTTLE(
                get_logger(), *get_clock(), 5000, "skipping a GNSS pose that is not finite"
            );
            return;
        }
        if (!this->trigger_client_->service_is_ready()) {
            RCLCPP_WARN_THROTTLE(
                get_logger(), *get_clock(), 5000, "waiting for the EKF's trigger_node service"
            );
            return;
        }
        this->initial_pose_pub_->publish(pose);
        auto request = std::make_shared<std_srvs::srv::SetBool::Request>();
        request->data = true;
        this->trigger_client_->async_send_request(
            request, [this](rclcpp::Client<std_srvs::srv::SetBool>::SharedFuture done) {
                if (done.get()->success) {
                    RCLCPP_INFO(get_logger(), "EKF started from the GNSS pose");
                } else {
                    RCLCPP_ERROR(get_logger(), "the EKF refused trigger_node");
                }
            }
        );
        this->initialized_ = true;
        RCLCPP_INFO(
            get_logger(),
            "initial pose from GNSS: x %.2f, y %.2f, z %.2f in %s",
            pose.pose.pose.position.x,
            pose.pose.pose.position.y,
            pose.pose.pose.position.z,
            pose.header.frame_id.c_str()
        );
    }

    /// Two checks in a row without odometry after the trigger, at least a second: start
    /// again from the next pose.
    void check_started()
    {
        const bool silent = this->initialized_ && !this->odometry_seen_;
        this->silent_checks_ = silent ? this->silent_checks_ + 1 : 0;
        this->odometry_seen_ = false;
        if (this->silent_checks_ >= 2) {
            RCLCPP_WARN(get_logger(), "no EKF odometry since the trigger; starting the EKF again");
            this->initialized_ = false;
            this->silent_checks_ = 0;
        }
    }

    static bool is_finite(const geometry_msgs::msg::PoseWithCovarianceStamped& pose)
    {
        const auto& p = pose.pose.pose.position;
        const auto& q = pose.pose.pose.orientation;
        for (const double value : {p.x, p.y, p.z, q.x, q.y, q.z, q.w}) {
            if (!std::isfinite(value)) {
                return false;
            }
        }
        return true;
    }

    bool initialized_{false};
    bool odometry_seen_{false};
    int  silent_checks_{0};
    rclcpp::Publisher<geometry_msgs::msg::PoseWithCovarianceStamped>::SharedPtr initial_pose_pub_;
    rclcpp::Client<std_srvs::srv::SetBool>::SharedPtr                           trigger_client_;
    rclcpp::Subscription<geometry_msgs::msg::PoseWithCovarianceStamped>::SharedPtr gnss_pose_sub_;
    rclcpp::Subscription<nav_msgs::msg::Odometry>::SharedPtr                       odometry_sub_;
    rclcpp::TimerBase::SharedPtr                                                   watchdog_;
};

}  // namespace gnss_pose_initializer

#include <rclcpp_components/register_node_macro.hpp>
RCLCPP_COMPONENTS_REGISTER_NODE(gnss_pose_initializer::GnssPoseInitializer)
