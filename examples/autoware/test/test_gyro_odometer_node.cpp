/// @file
/// The upstream GyroOdometerNode on the bus, through the rclcpp shim, on simulated time that
/// the test drives: the vehicle twist and the IMU in, the fused twist on its four topics
/// and the diagnostics out. The scenarios follow upstream's test_gyro_odometer_node.cpp,
/// which needs rclcpp executors: a completed pair, an IMU frame that cannot be resolved,
/// a completed fusion that reports OK, and an IMU mounted a quarter turn. Each sample is
/// served before the next is sent, so the node sees them in the scenario's order.

#include <hypha/transform_buffer.h>
#include <hypha_msgs/rosgraph_msgs/Clock.h>

#include "gyro_odometer_node.hpp"

#include <autoware_utils_geometry/msg/covariance.hpp>
#include <diagnostic_msgs/msg/diagnostic_array.hpp>
#include <geometry_msgs/msg/twist_stamped.hpp>
#include <geometry_msgs/msg/twist_with_covariance_stamped.hpp>
#include <gtest/gtest.h>
#include <rclcpp/rclcpp.hpp>
#include <sensor_msgs/msg/imu.hpp>

#include <memory>
#include <optional>
#include <string>

namespace {

constexpr const char* kParameterFile = "test/config/gyro_odometer_node_test.toml";
/// The diagnostics timer's period: one more step of /clock makes one report.
constexpr int64_t kDiagnosticsNanos = 100'000'000;

using COV_IDX_XYZ = autoware_utils_geometry::xyz_covariance_index::XYZ_COV_IDX;
using COV_IDX_XYZRPY = autoware_utils_geometry::xyzrpy_covariance_index::XYZRPY_COV_IDX;
using Imu = sensor_msgs::msg::Imu;
using Twist = geometry_msgs::msg::TwistStamped;
using TwistCov = geometry_msgs::msg::TwistWithCovarianceStamped;
using Status = diagnostic_msgs::msg::DiagnosticStatus;

class GyroOdometerNodeTest : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->gyro_ =
            std::make_shared<autoware::gyro_odometer::GyroOdometerNode>(rclcpp::NodeOptions{});
        this->harness_ = rclcpp::Node::make_shared("gyro_test_harness");
        hypha::Node& bus = this->harness_->hypha_node();
        this->clock_ = bus.advertise<rosgraph_msgs::Clock>(hypha::Topic{"/clock"});
        this->vehicleTwist_ = bus.advertise<TwistCov>(
            hypha::Topic{"/sensing/vehicle_velocity_converter/twist_with_covariance"}
        );
        this->imu_ = bus.advertise<Imu>(hypha::Topic{"/sensing/imu/imu_data"});
        this->tfStatic_.emplace(bus);
        const std::string ns = "/localization/twist_estimator/";
        this->subscriptions_.push_back(bus.subscribe<Twist>(
            hypha::Topic{ns + "gyro_twist_raw"}, [this](const Twist::ConstPtr& m) {
                this->twistRaw_ = *m;
            }
        ));
        this->subscriptions_.push_back(
            bus.subscribe<Twist>(hypha::Topic{ns + "gyro_twist"}, [this](const Twist::ConstPtr& m) {
                this->twist_ = *m;
            })
        );
        this->covSubscriptions_.push_back(bus.subscribe<TwistCov>(
            hypha::Topic{ns + "twist_with_covariance_raw"}, [this](const TwistCov::ConstPtr& m) {
                this->twistCovRaw_ = *m;
            }
        ));
        this->covSubscriptions_.push_back(bus.subscribe<TwistCov>(
            hypha::Topic{ns + "twist_with_covariance"}, [this](const TwistCov::ConstPtr& m) {
                this->twistCov_ = *m;
            }
        ));
        this->diagnosticsIn_ = bus.subscribe<diagnostic_msgs::msg::DiagnosticArray>(
            hypha::Topic{"/diagnostics"},
            [this](const diagnostic_msgs::msg::DiagnosticArray::ConstPtr& array) {
                for (const Status& status : array->status) {
                    if (status.name.find("gyro_odometer_status") != std::string::npos) {
                        this->level_ = status.level;
                    }
                }
            }
        );
        this->advance(1);
    }

    void turn()
    {
        for (int i = 0; i < 3; ++i) {
            rclcpp::spin_some(this->harness_);
            rclcpp::spin_some(this->gyro_);
        }
    }

    /// /clock @p nanos later. Simulated time belongs to the process and never goes back.
    void advance(int64_t nanos)
    {
        nowNanos_ += nanos;
        rosgraph_msgs::Clock clock;
        clock.clock = rclcpp::Time(nowNanos_);
        ASSERT_TRUE(this->clock_.publish(clock));
        this->turn();
    }

    builtin_interfaces::msg::Time stamp() const
    {
        return rclcpp::Time(nowNanos_);
    }

    void sendVehicleTwist(double vx, double covXx)
    {
        TwistCov twist;
        twist.header.stamp = this->stamp();
        twist.header.frame_id = "base_link";
        twist.twist.twist.linear.x = vx;
        twist.twist.covariance[COV_IDX_XYZRPY::X_X] = covXx;
        ASSERT_TRUE(this->vehicleTwist_.publish(twist));
        this->turn();
    }

    void sendImu(
        const std::string& frame,
        double             wx,
        double             wy,
        double             wz,
        double             cxx,
        double             cyy,
        double             czz
    )
    {
        Imu imu;
        imu.header.stamp = this->stamp();
        imu.header.frame_id = frame;
        imu.angular_velocity.x = wx;
        imu.angular_velocity.y = wy;
        imu.angular_velocity.z = wz;
        imu.angular_velocity_covariance[COV_IDX_XYZ::X_X] = cxx;
        imu.angular_velocity_covariance[COV_IDX_XYZ::Y_Y] = cyy;
        imu.angular_velocity_covariance[COV_IDX_XYZ::Z_Z] = czz;
        ASSERT_TRUE(this->imu_.publish(imu));
        this->turn();
    }

    /// The level of the next report.
    std::optional<uint8_t> diagnosticsLevel()
    {
        this->level_.reset();
        this->advance(kDiagnosticsNanos);
        return this->level_;
    }

    static inline int64_t                                      nowNanos_{100'000'000'000};
    std::shared_ptr<autoware::gyro_odometer::GyroOdometerNode> gyro_;
    rclcpp::Node::SharedPtr                                    harness_;
    hypha::Publisher<rosgraph_msgs::Clock>                     clock_;
    hypha::Publisher<TwistCov>                                 vehicleTwist_;
    hypha::Publisher<Imu>                                      imu_;
    std::optional<hypha::StaticTransformBroadcaster>           tfStatic_;
    std::vector<hypha::Subscriber<Twist>>                      subscriptions_;
    std::vector<hypha::Subscriber<TwistCov>>                   covSubscriptions_;
    hypha::Subscriber<diagnostic_msgs::msg::DiagnosticArray>   diagnosticsIn_;
    std::optional<Twist>                                       twistRaw_;
    std::optional<Twist>                                       twist_;
    std::optional<TwistCov>                                    twistCovRaw_;
    std::optional<TwistCov>                                    twistCov_;
    std::optional<uint8_t>                                     level_;
};

}  // namespace

// A completed pair puts the vehicle's longitudinal velocity and the IMU's angular velocity
// on all four topics; the angular variance is the largest axis's, on all three.
TEST_F(GyroOdometerNodeTest, ACompletedPairIsPublishedOnAllFourTopics)
{
    this->sendVehicleTwist(1.0, 4.0);
    this->sendImu("base_link", 0.1, 0.2, 0.3, 0.01, 0.02, 0.03);
    this->sendVehicleTwist(1.0, 4.0);

    ASSERT_TRUE(this->twistRaw_ && this->twist_ && this->twistCovRaw_ && this->twistCov_);
    EXPECT_DOUBLE_EQ(this->twistRaw_->twist.linear.x, 1.0);
    EXPECT_DOUBLE_EQ(this->twistRaw_->twist.angular.z, 0.3);
    EXPECT_DOUBLE_EQ(this->twist_->twist.linear.x, 1.0);
    EXPECT_DOUBLE_EQ(this->twist_->twist.angular.z, 0.3);
    const TwistCov& raw = *this->twistCovRaw_;
    EXPECT_EQ(raw.header.frame_id, "base_link");
    EXPECT_DOUBLE_EQ(raw.twist.twist.linear.x, 1.0);
    EXPECT_DOUBLE_EQ(raw.twist.twist.angular.x, 0.1);
    EXPECT_DOUBLE_EQ(raw.twist.twist.angular.y, 0.2);
    EXPECT_DOUBLE_EQ(raw.twist.twist.angular.z, 0.3);
    EXPECT_DOUBLE_EQ(raw.twist.covariance[COV_IDX_XYZRPY::Y_Y], 100000.0);
    EXPECT_DOUBLE_EQ(raw.twist.covariance[COV_IDX_XYZRPY::Z_Z], 100000.0);
    EXPECT_DOUBLE_EQ(raw.twist.covariance[COV_IDX_XYZRPY::ROLL_ROLL], 0.03);
    EXPECT_DOUBLE_EQ(raw.twist.covariance[COV_IDX_XYZRPY::PITCH_PITCH], 0.03);
    EXPECT_DOUBLE_EQ(raw.twist.covariance[COV_IDX_XYZRPY::YAW_YAW], 0.03);
    EXPECT_EQ(this->twistCov_->twist.covariance, raw.twist.covariance);
    EXPECT_EQ(this->diagnosticsLevel(), Status::OK);
}

// An IMU frame that cannot be resolved into base_link drops the data and reports ERROR.
TEST_F(GyroOdometerNodeTest, AnUnresolvableImuFrameDropsTheDataAndFails)
{
    this->sendVehicleTwist(1.0, 4.0);
    this->sendImu("imu_link", 0.1, 0.2, 0.3, 0.01, 0.01, 0.01);
    this->sendVehicleTwist(1.0, 4.0);

    EXPECT_FALSE(this->twistCovRaw_.has_value());
    EXPECT_EQ(this->diagnosticsLevel(), Status::ERROR);
}

// A quarter turn about z between imu_link and base_link on /tf_static, as upstream's
// scenario sends it (parent imu_link, child base_link): the node looks the transform up
// from imu_link to base_link and rotates the angular velocity by it, so (x, y, z) becomes
// (-y, x, z).
TEST_F(GyroOdometerNodeTest, AMountedImuIsRotatedIntoBaseLink)
{
    geometry_msgs::msg::TransformStamped mount;
    mount.header.frame_id = "imu_link";
    mount.child_frame_id = "base_link";
    mount.transform.rotation.z = 0.7071067811865476;
    mount.transform.rotation.w = 0.7071067811865476;
    ASSERT_TRUE(this->tfStatic_->send(mount));
    this->turn();

    this->sendVehicleTwist(1.0, 4.0);
    this->sendImu("imu_link", 0.1, 0.2, 0.3, 0.01, 0.02, 0.03);
    this->sendVehicleTwist(1.0, 4.0);

    ASSERT_TRUE(this->twistCovRaw_.has_value());
    EXPECT_NEAR(this->twistCovRaw_->twist.twist.angular.x, -0.2, 1e-9);
    EXPECT_NEAR(this->twistCovRaw_->twist.twist.angular.y, 0.1, 1e-9);
    EXPECT_NEAR(this->twistCovRaw_->twist.twist.angular.z, 0.3, 1e-9);
    EXPECT_EQ(this->twistCovRaw_->header.frame_id, "base_link");
}

int main(int argc, char** argv)
{
    ::testing::InitGoogleTest(&argc, argv);
    const char* hyphaArgv[] = {argv[0], kParameterFile};
    rclcpp::init(2, hyphaArgv);
    const int result = RUN_ALL_TESTS();
    rclcpp::shutdown();
    return result;
}
